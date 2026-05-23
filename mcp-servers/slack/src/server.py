# MCP Server for Slack Integration
# Provides: Message sending, Channel management, Notifications
# Version: 2.0 (85% feature complete)

from mcp.server import FastMCP
import httpx
import os
import logging
from typing import Optional

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

mcp = FastMCP("slack")

SLACK_BOT_TOKEN = os.getenv("SLACK_BOT_TOKEN", "")
SLACK_API = "https://slack.com/api"

if not SLACK_BOT_TOKEN:
    logger.warning("SLACK_BOT_TOKEN not set. Operations may fail.")

async def get_headers():
    return {
        "Authorization": f"Bearer {SLACK_BOT_TOKEN}",
        "Content-Type": "application/json"
    }

def handle_slack_error(resp: dict, operation: str) -> str:
    """Handle Slack API errors"""
    if not resp.get('ok'):
        error = resp.get('error', 'Unknown error')
        return f"Slack API error: {error}"
    return None

@mcp.tool()
async def send_message(channel: str, text: str) -> str:
    """Send a message to a Slack channel"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        data = {"channel": channel, "text": text}
        resp = await client.post(
            f"{SLACK_API}/chat.postMessage",
            headers=headers,
            json=data
        )
        result = resp.json()
        if result["ok"]:
            return f"Message sent to {channel}: {result['ts']}"
        return f"Failed: {result.get('error')}"

@mcp.tool()
async def send_dm(user_id: str, text: str) -> str:
    """Send a direct message to a user"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{SLACK_API}/conversations.open",
            headers=headers,
            json={"users": user_id}
        )
        dm_data = resp.json()
        if dm_data["ok"]:
            channel = dm_data["channel"]["id"]
            return await send_message(channel, text)
        return f"Failed: {dm_data.get('error')}"

@mcp.tool()
async def list_channels(limit: int = 100) -> str:
    """List public channels"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{SLACK_API}/conversations.list",
            headers=headers,
            params={"types": "public_channel", "limit": limit}
        )
        data = resp.json()
        channels = data.get("channels", [])
        result = [f"#{ch['name']} ({ch['num_members']} members)" for ch in channels]
        return "\n".join(result) if result else "No channels found"

@mcp.tool()
async def get_channel_history(channel: str, limit: int = 10) -> str:
    """Get recent messages from a channel"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{SLACK_API}/conversations.history",
            headers=headers,
            params={"channel": channel, "limit": limit}
        )
        data = resp.json()
        messages = data.get("messages", [])
        result = [f"{msg.get('user', 'Unknown')}: {msg.get('text', '')}" for msg in messages]
        return "\n".join(result) if result else "No messages found"

@mcp.tool()
async def add_reaction(channel: str, timestamp: str, emoji: str) -> str:
    """Add a reaction to a message"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{SLACK_API}/reactions.add",
            headers=headers,
            json={"channel": channel, "timestamp": timestamp, "name": emoji}
        )
        result = resp.json()
        error = handle_slack_error(result, "add_reaction")
        if error:
            return error
        return f"Added :{emoji}: reaction"

@mcp.tool()
async def create_channel(name: str, is_private: bool = False) -> str:
    """Create a new Slack channel
    
    Args:
        name: Channel name (no # prefix)
        is_private: Whether to create a private channel
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        visibility = "private" if is_private else "public"
        resp = await client.post(
            f"{SLACK_API}/conversations.create",
            headers=headers,
            json={"name": name, "is_private": is_private}
        )
        result = resp.json()
        error = handle_slack_error(result, "create_channel")
        if error:
            return error
        
        channel_id = result['channel']['id']
        return f"Created {'private' if is_private else 'public'} channel #{name} (ID: {channel_id})"

@mcp.tool()
async def invite_to_channel(channel: str, users: list) -> str:
    """Invite users to a channel
    
    Args:
        channel: Channel ID or name
        users: List of user IDs to invite
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        user_ids = ",".join(users)
        resp = await client.post(
            f"{SLACK_API}/conversations.invite",
            headers=headers,
            json={"channel": channel, "users": user_ids}
        )
        result = resp.json()
        error = handle_slack_error(result, "invite_to_channel")
        if error:
            return error
        
        return f"Invited {len(users)} users to channel"

@mcp.tool()
async def archive_channel(channel: str, confirm: bool = False) -> str:
    """Archive a channel. Requires confirm=True."""
    if not confirm:
        return f"WARNING: This will archive the channel. Set confirm=true to proceed."
    
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{SLACK_API}/conversations.archive",
            headers=headers,
            json={"channel": channel}
        )
        result = resp.json()
        error = handle_slack_error(result, "archive_channel")
        if error:
            return error
        
        return f"Channel {channel} archived"

@mcp.tool()
async def search_messages(query: str, count: int = 10) -> str:
    """Search for messages matching a query
    
    Args:
        query: Search query string
        count: Number of results to return (max 20)
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{SLACK_API}/search.messages",
            headers=headers,
            params={"query": query, "count": min(count, 20)}
        )
        result = resp.json()
        error = handle_slack_error(result, "search_messages")
        if error:
            return error
        
        messages = result.get('messages', {}).get('matches', [])
        if not messages:
            return "No messages found"
        
        lines = [f"Found {len(messages)} messages:"]
        for msg in messages:
            channel = msg.get('channel', {}).get('name', 'unknown')
            user = msg.get('user', 'unknown')
            text = msg.get('text', '')[:100]
            ts = msg.get('ts', '')
            lines.append(f"  #{channel} - {user}: {text}")
        
        return "\n".join(lines)

@mcp.tool()
async def get_user_info(user_id: str) -> str:
    """Get information about a Slack user"""
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.get(
            f"{SLACK_API}/users.info",
            headers=headers,
            params={"user": user_id}
        )
        result = resp.json()
        error = handle_slack_error(result, "get_user_info")
        if error:
            return error
        
        user = result.get('user', {})
        profile = user.get('profile', {})
        
        info = f"""User: {user.get('real_name', 'N/A')}
Username: @{user.get('name', 'N/A')}
Email: {profile.get('email', 'N/A')}
Title: {profile.get('title', 'N/A')}
Status: {profile.get('status_text', 'N/A')}"""
        
        return info

@mcp.tool()
async def update_message(channel: str, timestamp: str, text: str) -> str:
    """Update an existing message
    
    Args:
        channel: Channel ID
        timestamp: Message timestamp (ts)
        text: New message text
    """
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{SLACK_API}/chat.update",
            headers=headers,
            json={"channel": channel, "ts": timestamp, "text": text}
        )
        result = resp.json()
        error = handle_slack_error(result, "update_message")
        if error:
            return error
        
        return f"Message updated in {channel}"

@mcp.tool()
async def delete_message(channel: str, timestamp: str, confirm: bool = False) -> str:
    """Delete a message. Requires confirm=True."""
    if not confirm:
        return f"WARNING: This will permanently delete the message. Set confirm=true to proceed."
    
    async with httpx.AsyncClient() as client:
        headers = await get_headers()
        resp = await client.post(
            f"{SLACK_API}/chat.delete",
            headers=headers,
            json={"channel": channel, "ts": timestamp}
        )
        result = resp.json()
        error = handle_slack_error(result, "delete_message")
        if error:
            return error
        
        return f"Message deleted from {channel}"

if __name__ == "__main__":
    logger.info("Starting Slack MCP Server...")
    mcp.run()
