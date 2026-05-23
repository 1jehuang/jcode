"""
Redis MCP Server
================
Provides: Key-value CRUD, Key scanning, TTL management, Cache flushing, List/Set/Hash operations
via the Model Context Protocol (MCP).

Enhanced version with connection pooling, error handling, and comprehensive data structure support.

Environment:
  REDIS_URL - Redis connection string (e.g. redis://localhost:6379/0)
"""

from mcp.server import FastMCP
import redis.asyncio as redis
import os
import logging
from typing import Optional

# Configure logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

mcp = FastMCP("redis")
REDIS_URL = os.getenv("REDIS_URL", "redis://localhost:6379")

# Connection pool (initialized on first use)
_redis_pool = None


async def get_redis():
    """Get or create Redis connection pool."""
    global _redis_pool
    if _redis_pool is None:
        logger.info(f"Creating Redis connection pool for {REDIS_URL}")
        _redis_pool = redis.from_url(
            REDIS_URL,
            decode_responses=True,
            max_connections=10,
            retry_on_timeout=True,
            socket_keepalive=True
        )
    return _redis_pool


# ---------------------------------------------------------------------------
# 1. get_key
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_key(key: str) -> str:
    """Get the value of a key from Redis.

    Args:
        key: Redis key name
    """
    r = await get_redis()
    try:
        # Detect type and handle accordingly
        key_type = await r.type(key)
        if key_type == "none":
            return f"Key '{key}' not found."

        if key_type == "string":
            value = await r.get(key)
            return f"Key '{key}' (string): {value}"
        elif key_type == "list":
            length = await r.llen(key)
            values = await r.lrange(key, 0, -1)
            lines = [f"Key '{key}' (list, length={length}):"]
            for v in values[:50]:
                lines.append(f"  - {v}")
            if len(values) > 50:
                lines.append(f"  ... and {len(values) - 50} more")
            return "\n".join(lines)
        elif key_type == "set":
            members = await r.smembers(key)
            lines = [f"Key '{key}' (set, cardinality={len(members)}):"]
            for m in sorted(members)[:50]:
                lines.append(f"  - {m}")
            return "\n".join(lines)
        elif key_type == "hash":
            fields = await r.hgetall(key)
            lines = [f"Key '{key}' (hash, fields={len(fields)}):"]
            for k, v in list(fields.items())[:50]:
                lines.append(f"  {k}: {v}")
            return "\n".join(lines)
        elif key_type == "zset":
            members = await r.zrange(key, 0, -1, withscores=True)
            lines = [f"Key '{key}' (sorted set, cardinality={len(members)}):"]
            for m, score in list(members)[:50]:
                lines.append(f"  {m}: {score}")
            return "\n".join(lines)
        else:
            return f"Key '{key}' (type={key_type}) - use redis-cli to inspect"
    except Exception as e:
        logger.error(f"Error getting key '{key}': {e}")
        return f"Error getting key: {e}"


# ---------------------------------------------------------------------------
# 2. set_key
# ---------------------------------------------------------------------------
@mcp.tool()
async def set_key(key: str, value: str, ttl: int = 0) -> str:
    """Set a string value for a key in Redis.

    Args:
        key: Redis key name
        value: String value to store
        ttl: Time-to-live in seconds (0 = no expiry)
    """
    r = await get_redis()
    try:
        await r.set(key, value, ex=ttl if ttl > 0 else None)
        msg = f"Set key '{key}'"
        if ttl > 0:
            msg += f" (TTL: {ttl}s)"
        return msg
    except Exception as e:
        logger.error(f"Error setting key '{key}': {e}")
        return f"Error setting key: {e}"


# ---------------------------------------------------------------------------
# 3. delete_key
# ---------------------------------------------------------------------------
@mcp.tool()
async def delete_key(key: str) -> str:
    """Delete a key from Redis.

    Args:
        key: Redis key name to delete
    """
    r = await get_redis()
    try:
        result = await r.delete(key)
        if result:
            return f"Deleted key '{key}'"
        return f"Key '{key}' not found"
    except Exception as e:
        logger.error(f"Error deleting key '{key}': {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 4. list_keys
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_keys(pattern: str = "*") -> str:
    """List keys matching a pattern in Redis.

    Args:
        pattern: Redis key pattern (default: *)
    """
    r = await get_redis()
    try:
        keys = await r.keys(pattern)
        if not keys:
            return f"No keys found matching pattern '{pattern}'."
        # Get type for each key
        lines = [f"Keys matching '{pattern}' ({len(keys)} total):", ""]
        for key in sorted(keys)[:200]:
            key_type = await r.type(key)
            lines.append(f"  - {key:40s} ({key_type})")
        if len(keys) > 200:
            lines.append(f"  ... and {len(keys) - 200} more")
        return "\n".join(lines)
    except Exception as e:
        logger.error(f"Error listing keys: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 5. get_ttl
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_ttl(key: str) -> str:
    """Get the TTL (time-to-live) of a key.

    Args:
        key: Redis key name
    """
    r = await get_redis()
    try:
        ttl = await r.ttl(key)
        if ttl == -2:
            return f"Key '{key}' does not exist."
        elif ttl == -1:
            return f"Key '{key}' has no expiry (persistent)."
        else:
            return f"Key '{key}' TTL: {ttl}s"
    except Exception as e:
        logger.error(f"Error getting TTL for '{key}': {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 6. flush_db
# ---------------------------------------------------------------------------
@mcp.tool()
async def flush_db(confirm: bool = False) -> str:
    """Flush (delete all keys in) the current Redis database.
    Requires confirm=True to execute.

    Args:
        confirm: Must be true to actually flush
    """
    if not confirm:
        return "WARNING: This will delete ALL keys. Set confirm=true to proceed."

    r = await get_redis()
    try:
        await r.flushdb()
        return "Redis database flushed (all keys deleted)."
    except Exception as e:
        logger.error(f"Error flushing database: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 7. push_to_list
# ---------------------------------------------------------------------------
@mcp.tool()
async def push_to_list(key: str, value: str, push_left: bool = False) -> str:
    """Push a value to a Redis list.

    Args:
        key: Redis list key name
        value: Value to push
        push_left: If true, push to left (LPUSH), otherwise right (RPUSH)
    """
    r = await get_redis()
    try:
        if push_left:
            await r.lpush(key, value)
        else:
            await r.rpush(key, value)
        length = await r.llen(key)
        return f"Pushed to list '{key}' (new length: {length})"
    except Exception as e:
        logger.error(f"Error pushing to list: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 8. add_to_set
# ---------------------------------------------------------------------------
@mcp.tool()
async def add_to_set(key: str, member: str) -> str:
    """Add a member to a Redis set.

    Args:
        key: Redis set key name
        member: Member to add
    """
    r = await get_redis()
    try:
        result = await r.sadd(key, member)
        cardinality = await r.scard(key)
        return f"Added '{member}' to set '{key}' (cardinality: {cardinality})"
    except Exception as e:
        logger.error(f"Error adding to set: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 9. hash_set
# ---------------------------------------------------------------------------
@mcp.tool()
async def hash_set(key: str, field: str, value: str) -> str:
    """Set a field in a Redis hash.

    Args:
        key: Redis hash key name
        field: Hash field name
        value: Field value
    """
    r = await get_redis()
    try:
        await r.hset(key, field, value)
        return f"Set hash '{key}' field '{field}'"
    except Exception as e:
        logger.error(f"Error setting hash field: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 10. ping
# ---------------------------------------------------------------------------
@mcp.tool()
async def ping() -> str:
    """Check if Redis server is responsive."""
    r = await get_redis()
    try:
        pong = await r.ping()
        info = await r.info("server")
        redis_version = info.get("redis_version", "unknown")
        return f"Redis PONG (version: {redis_version})"
    except Exception as e:
        logger.error(f"Error pinging Redis: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 11. set_expiry
# ---------------------------------------------------------------------------
@mcp.tool()
async def set_expiry(key: str, ttl: int) -> str:
    """Set or update the TTL on a key.

    Args:
        key: Redis key name
        ttl: Time-to-live in seconds (-1 to remove expiry)
    """
    r = await get_redis()
    try:
        if ttl == -1:
            await r.persist(key)
            return f"Removed expiry from key '{key}'"
        else:
            await r.expire(key, ttl)
            return f"Set TTL on key '{key}' to {ttl}s"
    except Exception as e:
        logger.error(f"Error setting expiry: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 12. increment
# ---------------------------------------------------------------------------
@mcp.tool()
async def increment(key: str, amount: int = 1) -> str:
    """Increment a numeric key's value.

    Args:
        key: Redis key name (must contain integer)
        amount: Amount to increment by (default: 1)
    """
    r = await get_redis()
    try:
        new_value = await r.incrby(key, amount)
        return f"Key '{key}' incremented by {amount}, new value: {new_value}"
    except Exception as e:
        logger.error(f"Error incrementing key: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 13. pop_from_list
# ---------------------------------------------------------------------------
@mcp.tool()
async def pop_from_list(key: str, pop_left: bool = True) -> str:
    """Pop an element from a Redis list.

    Args:
        key: Redis list key name
        pop_left: If true, pop from left (LPOP), otherwise right (RPOP)
    """
    r = await get_redis()
    try:
        if pop_left:
            value = await r.lpop(key)
        else:
            value = await r.rpop(key)
        
        if value is None:
            return f"List '{key}' is empty or doesn't exist"
        
        length = await r.llen(key)
        return f"Popped '{value}' from list '{key}' (remaining: {length})"
    except Exception as e:
        logger.error(f"Error popping from list: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# 14. get_memory_info
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_memory_info() -> str:
    """Get Redis memory usage information."""
    r = await get_redis()
    try:
        info = await r.info("memory")
        used_human = info.get('used_memory_human', 'N/A')
        peak_human = info.get('used_memory_peak_human', 'N/A')
        fragmentation_ratio = info.get('mem_fragmentation_ratio', 'N/A')
        
        return f"""Redis Memory Usage:
  Used: {used_human}
  Peak: {peak_human}
  Fragmentation Ratio: {fragmentation_ratio}"""
    except Exception as e:
        logger.error(f"Error getting memory info: {e}")
        return f"Error: {e}"


if __name__ == "__main__":
    logger.info("Starting Redis MCP Server...")
    mcp.run()
