# Datadog MCP Server

MCP server for Datadog monitoring and observability.

## Features

- **10+ Tools** for monitoring
- Metric queries and dashboards
- Log search and analysis
- Monitor management
- Service health checks

## Configuration

| Variable | Description | Example |
|----------|-------------|---------|
| `DD_API_KEY` | Datadog API key | `abc123...` |
| `DD_APP_KEY` | Datadog Application key | `xyz789...` |
| `DD_SITE` | Datadog site | `datadoghq.com` |

## Available Tools

1. `query_metrics(query, from_ts, to_ts)` - Query metrics
2. `list_monitors(tags = [])` - List monitors
3. `get_monitor(monitor_id)` - Get monitor details
4. `mute_monitor(monitor_id)` - Mute a monitor
5. `unmute_monitor(monitor_id)` - Unmute a monitor
6. `search_logs(query, limit = 100)` - Search logs
7. `list_dashboards()` - List dashboards
8. `get_dashboard(dashboard_id)` - Get dashboard details
9. `get_service_health(service)` - Check service health
10. `get_slo_status(slo_id)` - Get SLO status

## Testing

```bash
pytest tests/test_datadog_mcp.py -v
```

## License

MIT
