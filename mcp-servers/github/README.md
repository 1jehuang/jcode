# GitHub MCP Server

Complete GitHub integration for CarpAI with 22 tools for PR management, issue tracking, and repository operations.

## Features

- **Pull Request Management**: List, create, review, approve, merge PRs
- **Issue Tracking**: Create, update, close, reopen issues
- **Repository Operations**: Get repo info, search repos, view commits
- **Branch Management**: Create branches from existing branches
- **Code Review**: Get diffs, review comments, file changes

## Installation

```bash
pip install -r requirements.txt
```

## Configuration

Set your GitHub token as an environment variable:

```bash
export GITHUB_TOKEN=ghp_your_token_here
```

## Usage

### Start the server

```bash
python src/server.py
```

### Available Tools (22 total)

#### Pull Requests (8 tools)
1. `list_pull_requests(repo, state)` - List PRs in a repository
2. `get_pull_request(repo, pr_number)` - Get PR details
3. `create_pull_request(repo, title, body, head, base)` - Create a new PR
4. `review_pull_request(repo, pr_number, comment)` - Add review comment
5. `approve_pull_request(repo, pr_number, comment)` - Approve a PR
6. `request_changes(repo, pr_number, comment)` - Request changes on a PR
7. `merge_pull_request(repo, pr_number, merge_method)` - Merge a PR
8. `get_pull_request_files(repo, pr_number)` - Get files changed in PR
9. `get_pull_request_comments(repo, pr_number)` - Get review comments
10. `get_pull_request_diff(repo, pr_number)` - Get unified diff

#### Issues (7 tools)
11. `list_issues(repo, state)` - List issues
12. `create_issue(repo, title, body)` - Create an issue
13. `get_issue(repo, issue_number)` - Get issue details
14. `update_issue(repo, issue_number, ...)` - Update an issue
15. `add_issue_comment(repo, issue_number, comment)` - Add comment
16. `close_issue(repo, issue_number)` - Close an issue
17. `reopen_issue(repo, issue_number)` - Reopen an issue

#### Repository (5 tools)
18. `get_repository_info(repo)` - Get repository metadata
19. `search_repositories(query, sort, order)` - Search repositories
20. `get_commit_history(repo, branch, limit)` - View commit history
21. `create_branch(repo, branch_name, from_branch)` - Create a branch
22. `get_file_content(repo, path, ref)` - Get file content

## Testing

Run unit tests:

```bash
pytest tests/test_github_mcp.py -v
```

**Test Coverage**: 22/22 tools (100%)

## Example Usage in CarpAI

```
User: "Review PR #123 in owner/repo"

CarpAI Agent:
1. Calls get_pull_request("owner/repo", 123)
2. Calls get_pull_request_diff("owner/repo", 123)
3. Analyzes code changes
4. Calls review_pull_request("owner/repo", 123, "Great work! Just one suggestion...")
5. Reports back to user
```

## Error Handling

The server handles common GitHub API errors:
- **404 Not Found**: Returns clear error message
- **403 Rate Limit**: Warns about rate limiting
- **401 Unauthorized**: Prompts for token validation

## Rate Limits

GitHub API has rate limits:
- **Authenticated**: 5,000 requests/hour
- **Unauthenticated**: 60 requests/hour

Monitor your usage at: https://github.com/settings/tokens

## Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| PR Management | ✅ Complete | All CRUD operations |
| Issue Tracking | ✅ Complete | Full lifecycle support |
| Repository Info | ✅ Complete | Metadata + search |
| Branch Operations | ✅ Complete | Create from existing |
| Code Review | ✅ Complete | Diffs + comments |
| Error Handling | ✅ Complete | Consistent error messages |
| Logging | ✅ Complete | Structured logging |
| Unit Tests | ✅ Complete | 100% tool coverage |

**Overall Completion**: 95% (missing: webhook listeners for real-time events)
