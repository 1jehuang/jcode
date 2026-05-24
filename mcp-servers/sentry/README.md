# Sentry MCP Server

MCP server for Sentry error tracking integration.

## Features

- **10+ Tools** for error monitoring
- Issue listing and details
- Release management
- Project statistics
- Event retrieval

## Configuration

| Variable | Description | Example |
|----------|-------------|---------|
| `SENTRY_ORG` | Sentry organization slug | `my-org` |
| `SENTRY_AUTH_TOKEN` | Auth token from Sentry | `sntrys_...` |
| `SENTRY_PROJECT` | Default project name | `my-project` |

## Available Tools

1. `list_issues(project, status = "unresolved")` - List issues
2. `get_issue_details(issue_id)` - Get issue details
3. `resolve_issue(issue_id)` - Resolve an issue
4. `list_releases(project)` - List releases
5. `create_release(project, version)` - Create release
6. `get_project_stats(project)` - Get event statistics
7. `list_events(issue_id, limit = 10)` - List events
8. `get_event_details(event_id)` - Get event details
9. `list_projects()` - List all projects
10. `get_user_feedback(project)` - Get user feedback

## Testing

```bash
pytest tests/test_sentry_mcp.py -v
```

## License

MIT
