# Slack MCP Server

A Model Context Protocol (MCP) server for Slack messaging and channel management.

## Features

- **10+ Tools** for Slack integration
- Send messages to channels
- List channels and members
- Get message history
- Update/delete messages
- Error handling with Slack API error codes

## Configuration

| Variable | Description | Example |
|----------|-------------|---------|
| `SLACK_BOT_TOKEN` | Bot User OAuth Token | `xoxb-...` |

### Get Bot Token

1. Go to https://api.slack.com/apps
2. Create a new app or select existing
3. Add "chat:write" scope
4. Install to workspace and copy token

## Available Tools

1. `send_message(channel, text)` - Send message to channel
2. `list_channels(limit = 100)` - List all channels
3. `get_channel_info(channel_id)` - Get channel details
4. `get_messages(channel, limit = 10)` - Get recent messages
5. `update_message(channel, ts, text)` - Update existing message
6. `delete_message(channel, ts)` - Delete a message
7. `add_reaction(channel, ts, emoji)` - Add emoji reaction
8. `get_users(limit = 100)` - List workspace users
9. `get_user_info(user_id)` - Get user profile
10. `send_dm(user_id, text)` - Send direct message

## Testing

```bash
pytest tests/test_slack_mcp.py -v
```

## License

MIT
