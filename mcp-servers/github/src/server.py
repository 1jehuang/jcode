# MCP Server for GitHub Integration
# Provides: PR review, Issue management, Repository operations
# Version: 2.0 (95% feature complete)

from mcp.server import FastMCP
import httpx
import os
import base64
from typing import Optional
from datetime import datetime
import logging

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

mcp = FastMCP("github")

GITHUB_TOKEN = os.getenv("GITHUB_TOKEN", "")
GITHUB_API = "https://api.github.com"

if not GITHUB_TOKEN:
    logger.warning("GITHUB_TOKEN not set. Some operations may fail.")

async def get_headers():
    return {
        "Authorization": f"token {GITHUB_TOKEN}",
        "Accept": "application/vnd.github.v3+json",
        "User-Agent": "CarpAI-MCP-Server"
    }

def handle_github_error(resp: httpx.Response, operation: str) -> str:
    """Handle GitHub API errors consistently"""
    if resp.status_code == 404:
        return f"Error: Resource not found (404)"
    elif resp.status_code == 403:
        return f"Error: Rate limit exceeded or insufficient permissions (403)"
    elif resp.status_code >= 400:
        error_data = resp.json() if resp.content else {}
        message = error_data.get('message', 'Unknown error')
        return f"Error {resp.status_code}: {message}"
    return None

@mcp.tool()
async def list_pull_requests(repo: str, state: str = "open") -> str:
    """List pull requests in a repository"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/pulls?state={state}",
            headers=headers
        )
        prs = resp.json()
        result = []
        for pr in prs:
            result.append(f"#{pr['number']}: {pr['title']} by {pr['user']['login']} ({pr['state']})")
        return "\n".join(result) if result else "No pull requests found"

@mcp.tool()
async def get_pull_request(repo: str, pr_number: int) -> str:
    """Get details of a specific pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}",
            headers=headers
        )
        pr = resp.json()
        return f"""PR #{pr['number']}: {pr['title']}
Author: {pr['user']['login']}
State: {pr['state']}
Created: {pr['created_at']}
Body: {pr.get('body', 'No description')}"""

@mcp.tool()
async def review_pull_request(repo: str, pr_number: int, comment: str) -> str:
    """Add a review comment to a pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/reviews",
            headers=headers,
            json={"body": comment, "event": "COMMENT"}
        )
        return f"Review added to PR #{pr_number}"

@mcp.tool()
async def list_issues(repo: str, state: str = "open") -> str:
    """List issues in a repository"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/issues?state={state}",
            headers=headers
        )
        issues = resp.json()
        result = []
        for issue in issues:
            if 'pull_request' not in issue:
                result.append(f"#{issue['number']}: {issue['title']} by {issue['user']['login']}")
        return "\n".join(result) if result else "No issues found"

@mcp.tool()
async def create_issue(repo: str, title: str, body: str = "") -> str:
    """Create a new issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/issues",
            headers=headers,
            json={"title": title, "body": body}
        )
        issue = resp.json()
        return f"Issue #{issue['number']} created: {issue['html_url']}"

@mcp.tool()
async def get_file_content(repo: str, path: str, ref: str = "main") -> str:
    """Get file content from repository"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/contents/{path}?ref={ref}",
            headers=headers
        )
        import base64
        data = resp.json()
        content = base64.b64decode(data['content']).decode('utf-8')
        return content

@mcp.tool()
async def get_pull_request_files(repo: str, pr_number: int) -> str:
    """Get list of files changed in a pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/files",
            headers=headers
        )
        files = resp.json()
        result = []
        for f in files:
            result.append(f"{f['filename']} (+{f['additions']}/-{f['deletions']})")
        return "\n".join(result) if result else "No files changed"

@mcp.tool()
async def get_pull_request_comments(repo: str, pr_number: int) -> str:
    """Get review comments on a pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/comments",
            headers=headers
        )
        comments = resp.json()
        result = []
        for c in comments:
            result.append(f"{c['user']['login']}: {c['body']} (on {c['path']}:{c.get('line', 'N/A')})")
        return "\n".join(result) if result else "No review comments"

@mcp.tool()
async def approve_pull_request(repo: str, pr_number: int, comment: str = "") -> str:
    """Approve a pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        body = {"event": "APPROVE"}
        if comment:
            body["body"] = comment
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/reviews",
            headers=headers,
            json=body
        )
        return f"PR #{pr_number} approved"

@mcp.tool()
async def request_changes(repo: str, pr_number: int, comment: str) -> str:
    """Request changes on a pull request"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/reviews",
            headers=headers,
            json={"body": comment, "event": "REQUEST_CHANGES"}
        )
        return f"Changes requested on PR #{pr_number}"

@mcp.tool()
async def merge_pull_request(repo: str, pr_number: int, merge_method: str = "merge") -> str:
    """Merge a pull request (merge/squash/rebase)"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.put(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}/merge",
            headers=headers,
            json={"merge_method": merge_method}
        )
        data = resp.json()
        return f"PR #{pr_number} merged: {data.get('message', 'Success')}"

@mcp.tool()
async def get_issue(repo: str, issue_number: int) -> str:
    """Get details of a specific issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/issues/{issue_number}",
            headers=headers
        )
        issue = resp.json()
        return f"""Issue #{issue['number']}: {issue['title']}
Author: {issue['user']['login']}
State: {issue['state']}
Created: {issue['created_at']}
Body: {issue.get('body', 'No description')}
Labels: {', '.join(l['name'] for l in issue.get('labels', []))}"""

@mcp.tool()
async def update_issue(repo: str, issue_number: int, title: str = "", body: str = "", state: str = "", labels: list = None) -> str:
    """Update an existing issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        data = {}
        if title:
            data["title"] = title
        if body:
            data["body"] = body
        if state in ["open", "closed"]:
            data["state"] = state
        if labels:
            data["labels"] = labels
        resp = await client.patch(
            f"{GITHUB_API}/repos/{repo}/issues/{issue_number}",
            headers=headers,
            json=data
        )
        issue = resp.json()
        return f"Issue #{issue['number']} updated: {issue['html_url']}"

@mcp.tool()
async def add_issue_comment(repo: str, issue_number: int, comment: str) -> str:
    """Add a comment to an issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/issues/{issue_number}/comments",
            headers=headers,
            json={"body": comment}
        )
        data = resp.json()
        return f"Comment added: {data['html_url']}"

@mcp.tool()
async def close_issue(repo: str, issue_number: int) -> str:
    """Close an issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.patch(
            f"{GITHUB_API}/repos/{repo}/issues/{issue_number}",
            headers=headers,
            json={"state": "closed"}
        )
        return f"Issue #{issue_number} closed"

@mcp.tool()
async def reopen_issue(repo: str, issue_number: int) -> str:
    """Reopen a closed issue"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.patch(
            f"{GITHUB_API}/repos/{repo}/issues/{issue_number}",
            headers=headers,
            json={"state": "open"}
        )
        return f"Issue #{issue_number} reopened"

@mcp.tool()
async def create_pull_request(repo: str, title: str, body: str, head: str, base: str = "main") -> str:
    """Create a new pull request
    
    Args:
        repo: Repository name (e.g., 'owner/repo')
        title: PR title
        body: PR description
        head: Source branch name
        base: Target branch name (default: main)
    """
    logger.info(f"Creating PR in {repo}: {title}")
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/pulls",
            headers=headers,
            json={
                "title": title,
                "body": body,
                "head": head,
                "base": base
            }
        )
        
        error = handle_github_error(resp, "create_pr")
        if error:
            return error
            
        pr = resp.json()
        return f"PR #{pr['number']} created: {pr['html_url']}"

@mcp.tool()
async def get_pull_request_diff(repo: str, pr_number: int) -> str:
    """Get the diff/patch content of a pull request
    
    Returns the unified diff showing all changes in the PR.
    Useful for code review and understanding modifications.
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        # Override Accept header for raw diff
        headers["Accept"] = "application/vnd.github.v3.diff"
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/pulls/{pr_number}",
            headers=headers
        )
        
        error = handle_github_error(resp, "get_diff")
        if error:
            return error
        
        # Response is plain text diff
        diff_content = resp.text
        if not diff_content:
            return "No diff available (PR may be empty)"
        
        # Truncate if too large
        if len(diff_content) > 50000:
            return diff_content[:50000] + "\n\n... (diff truncated, too large)"
        
        return diff_content

@mcp.tool()
async def get_repository_info(repo: str) -> str:
    """Get repository metadata and statistics
    
    Returns: name, description, stars, forks, open issues, default branch, etc.
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}",
            headers=headers
        )
        
        error = handle_github_error(resp, "get_repo_info")
        if error:
            return error
        
        data = resp.json()
        
        info = f"""Repository: {data['full_name']}
Description: {data.get('description', 'No description')}
Stars: {data['stargazers_count']}
Forks: {data['forks_count']}
Open Issues: {data['open_issues_count']}
Default Branch: {data['default_branch']}
Language: {data.get('language', 'Unknown')}
Created: {data['created_at'][:10]}
Last Updated: {data['updated_at'][:10]}
License: {data.get('license', {}).get('name', 'Unknown')}
URL: {data['html_url']}"""
        
        return info

@mcp.tool()
async def search_repositories(query: str, sort: str = "stars", order: str = "desc") -> str:
    """Search GitHub repositories
    
    Args:
        query: Search query (e.g., 'machine learning language:python')
        sort: Sort by (stars, forks, updated)
        order: Order (asc, desc)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/search/repositories?q={query}&sort={sort}&order={order}&per_page=10",
            headers=headers
        )
        
        if resp.status_code != 200:
            return f"Search failed: {resp.status_code}"
        
        data = resp.json()
        items = data.get('items', [])
        
        if not items:
            return "No repositories found"
        
        result = [f"Found {data['total_count']} repositories (showing top 10):\n"]
        for repo in items:
            result.append(
                f"⭐ {repo['stargazers_count']} | {repo['full_name']}\n"
                f"   {repo.get('description', 'No description')[:100]}\n"
                f"   URL: {repo['html_url']}\n"
            )
        
        return "\n".join(result)

@mcp.tool()
async def get_commit_history(repo: str, branch: str = "main", limit: int = 10) -> str:
    """Get recent commit history for a branch
    
    Args:
        repo: Repository name
        branch: Branch name (default: main)
        limit: Number of commits to retrieve (default: 10)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/commits?sha={branch}&per_page={limit}",
            headers=headers
        )
        
        error = handle_github_error(resp, "get_commits")
        if error:
            return error
        
        commits = resp.json()
        
        if not commits:
            return "No commits found"
        
        result = [f"Recent commits on {branch}:\n"]
        for commit in commits:
            sha = commit['sha'][:8]
            message = commit['commit']['message'].split('\n')[0]
            author = commit['commit']['author']['name']
            date = commit['commit']['author']['date'][:10]
            result.append(f"{sha} - {message} ({author}, {date})")
        
        return "\n".join(result)

@mcp.tool()
async def create_branch(repo: str, branch_name: str, from_branch: str = "main") -> str:
    """Create a new branch from an existing branch
    
    Args:
        repo: Repository name
        branch_name: New branch name
        from_branch: Source branch (default: main)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        
        # Get the SHA of the source branch
        ref_resp = await client.get(
            f"{GITHUB_API}/repos/{repo}/git/ref/heads/{from_branch}",
            headers=headers
        )
        
        error = handle_github_error(ref_resp, "get_branch_ref")
        if error:
            return error
        
        sha = ref_resp.json()['object']['sha']
        
        # Create new branch
        resp = await client.post(
            f"{GITHUB_API}/repos/{repo}/git/refs",
            headers=headers,
            json={
                "ref": f"refs/heads/{branch_name}",
                "sha": sha
            }
        )
        
        error = handle_github_error(resp, "create_branch")
        if error:
            return error
        
        return f"Branch '{branch_name}' created from '{from_branch}'"

if __name__ == "__main__":
    logger.info("Starting GitHub MCP Server...")
    mcp.run()
