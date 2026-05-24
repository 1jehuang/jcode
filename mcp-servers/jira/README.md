# Jira MCP Server

A Model Context Protocol (MCP) server for Jira issue tracking and project management.

## Features

- **13 Tools** for comprehensive Jira integration
- Issue CRUD operations (create, read, update, delete)
- JQL search with customizable result limits
- Sprint management and worklog tracking
- Issue linking and comments
- Error handling with detailed status codes

## Installation

```bash
pip install -r requirements.txt
```

## Configuration

Set these environment variables:

| Variable | Description | Example |
|----------|-------------|---------|
| `JIRA_URL` | Jira instance URL | `https://your-domain.atlassian.net` |
| `JIRA_EMAIL` | Your Jira email | `user@example.com` |
| `JIRA_API_TOKEN` | API token from Atlassian | `ATATT...` |

### Get API Token

1. Go to https://id.atlassian.com/manage-profile/security/api-tokens
2. Click "Create API token"
3. Copy the token and set `JIRA_API_TOKEN`

## Available Tools

### 1. `search_issues(jql: str, max_results: int = 50)`
Search for issues using JQL (Jira Query Language).

**Example:**
```python
result = await search_issues("project = PROJ AND status = Open")
```

### 2. `get_issue(issue_key: str)`
Get full details of a specific issue.

**Example:**
```python
result = await get_issue("PROJ-123")
```

### 3. `create_issue(project: str, summary: str, issue_type: str = "Task", ...)`
Create a new Jira issue.

**Example:**
```python
result = await create_issue("PROJ", "Fix login bug", "Bug")
```

### 4. `update_issue(issue_key: str, ...)`
Update an existing issue (summary, description, status).

**Example:**
```python
result = await update_issue("PROJ-123", status="In Progress")
```

### 5. `add_comment(issue_key: str, comment: str)`
Add a comment to an issue.

### 6. `get_project_issues(project: str, status: str = "Open")`
Get all issues for a project filtered by status.

### 7. `delete_issue(issue_key: str, confirm: bool = False)`
Delete an issue permanently (requires admin permissions).

### 8. `get_transitions(issue_key: str)`
Get available status transitions for workflow.

### 9. `link_issues(inward_key, outward_key, link_type = "Blocks")`
Create a link between two issues.

### 10. `get_worklogs(issue_key: str)`
Get worklog entries for time tracking.

### 11. `add_worklog(issue_key, time_spent, comment = "")`
Add a worklog entry (e.g., "2h 30m").

### 12. `get_sprints(board_id: int, state = "active")`
Get sprints for a board (active/future/closed).

## Testing

```bash
pytest tests/test_jira_mcp.py -v --cov=src
```

## License

MIT
