"""
Redis MCP Server - Unit Tests
==============================
Tests for all 14 tools with comprehensive coverage.
"""

import pytest
import asyncio
from unittest.mock import AsyncMock, patch


class TestRedisMCP:
    """Test suite for Redis MCP server tools."""

    @pytest.fixture
    def mock_redis(self):
        """Create a mock Redis client."""
        redis_client = AsyncMock()
        return redis_client

    # ------------------------------------------------------------------
    # Tool 1: get_key
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_key_string(self, mock_redis):
        """Test getting a string value."""
        mock_redis.type = AsyncMock(return_value="string")
        mock_redis.get = AsyncMock(return_value="hello")

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_key
            result = await get_key("mykey")

        assert "mykey" in result
        assert "hello" in result

    @pytest.mark.asyncio
    async def test_get_key_list(self, mock_redis):
        """Test getting a list value."""
        mock_redis.type = AsyncMock(return_value="list")
        mock_redis.llen = AsyncMock(return_value=3)
        mock_redis.lrange = AsyncMock(return_value=["a", "b", "c"])

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_key
            result = await get_key("mylist")

        assert "list" in result
        assert "length=3" in result
        assert "a" in result

    @pytest.mark.asyncio
    async def test_get_key_not_found(self, mock_redis):
        """Test getting a non-existent key."""
        mock_redis.type = AsyncMock(return_value="none")

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_key
            result = await get_key("missing")

        assert "not found" in result

    # ------------------------------------------------------------------
    # Tool 2: set_key
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_set_key_without_ttl(self, mock_redis):
        """Test setting a key without TTL."""
        mock_redis.set = AsyncMock(return_value=True)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import set_key
            result = await set_key("mykey", "value")

        assert "Set key 'mykey'" in result
        mock_redis.set.assert_called_once_with("mykey", "value", ex=None)

    @pytest.mark.asyncio
    async def test_set_key_with_ttl(self, mock_redis):
        """Test setting a key with TTL."""
        mock_redis.set = AsyncMock(return_value=True)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import set_key
            result = await set_key("mykey", "value", ttl=60)

        assert "TTL: 60s" in result
        mock_redis.set.assert_called_once_with("mykey", "value", ex=60)

    # ------------------------------------------------------------------
    # Tool 3: delete_key
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_delete_key_success(self, mock_redis):
        """Test deleting an existing key."""
        mock_redis.delete = AsyncMock(return_value=1)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import delete_key
            result = await delete_key("mykey")

        assert "Deleted key 'mykey'" in result

    @pytest.mark.asyncio
    async def test_delete_key_not_found(self, mock_redis):
        """Test deleting a non-existent key."""
        mock_redis.delete = AsyncMock(return_value=0)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import delete_key
            result = await delete_key("missing")

        assert "not found" in result

    # ------------------------------------------------------------------
    # Tool 4: list_keys
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_list_keys_with_pattern(self, mock_redis):
        """Test listing keys matching a pattern."""
        mock_redis.keys = AsyncMock(return_value=["user:1", "user:2", "post:1"])
        mock_redis.type = AsyncMock(return_value="string")

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import list_keys
            result = await list_keys(pattern="user:*")

        assert "Keys matching 'user:*'" in result
        assert "user:1" in result
        assert "user:2" in result

    # ------------------------------------------------------------------
    # Tool 5: get_ttl
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_ttl_with_expiry(self, mock_redis):
        """Test getting TTL of a key with expiry."""
        mock_redis.ttl = AsyncMock(return_value=3600)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_ttl
            result = await get_ttl("mykey")

        assert "TTL: 3600s" in result

    @pytest.mark.asyncio
    async def test_get_ttl_no_expiry(self, mock_redis):
        """Test getting TTL of a persistent key."""
        mock_redis.ttl = AsyncMock(return_value=-1)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_ttl
            result = await get_ttl("mykey")

        assert "no expiry" in result

    @pytest.mark.asyncio
    async def test_get_ttl_key_not_exists(self, mock_redis):
        """Test getting TTL of a non-existent key."""
        mock_redis.ttl = AsyncMock(return_value=-2)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_ttl
            result = await get_ttl("missing")

        assert "does not exist" in result

    # ------------------------------------------------------------------
    # Tool 6: flush_db
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_flush_db_without_confirm(self, mock_redis):
        """Test flush_db without confirmation."""
        with patch('server.get_redis'):
            from server import flush_db
            result = await flush_db(confirm=False)

        assert "WARNING" in result
        assert "confirm=true" in result

    @pytest.mark.asyncio
    async def test_flush_db_with_confirm(self, mock_redis):
        """Test flush_db with confirmation."""
        mock_redis.flushdb = AsyncMock(return_value=True)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import flush_db
            result = await flush_db(confirm=True)

        assert "flushed" in result
        mock_redis.flushdb.assert_called_once()

    # ------------------------------------------------------------------
    # Tool 7: push_to_list
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_push_to_list_right(self, mock_redis):
        """Test pushing to the right of a list."""
        mock_redis.rpush = AsyncMock(return_value=5)
        mock_redis.llen = AsyncMock(return_value=5)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import push_to_list
            result = await push_to_list("mylist", "value", push_left=False)

        assert "Pushed to list 'mylist'" in result
        assert "new length: 5" in result

    @pytest.mark.asyncio
    async def test_push_to_list_left(self, mock_redis):
        """Test pushing to the left of a list."""
        mock_redis.lpush = AsyncMock(return_value=5)
        mock_redis.llen = AsyncMock(return_value=5)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import push_to_list
            result = await push_to_list("mylist", "value", push_left=True)

        mock_redis.lpush.assert_called_once()

    # ------------------------------------------------------------------
    # Tool 8: add_to_set
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_add_to_set(self, mock_redis):
        """Test adding a member to a set."""
        mock_redis.sadd = AsyncMock(return_value=1)
        mock_redis.scard = AsyncMock(return_value=10)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import add_to_set
            result = await add_to_set("myset", "member1")

        assert "Added 'member1' to set 'myset'" in result
        assert "cardinality: 10" in result

    # ------------------------------------------------------------------
    # Tool 9: hash_set
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_hash_set(self, mock_redis):
        """Test setting a field in a hash."""
        mock_redis.hset = AsyncMock(return_value=1)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import hash_set
            result = await hash_set("myhash", "field1", "value1")

        assert "Set hash 'myhash' field 'field1'" in result

    # ------------------------------------------------------------------
    # Tool 10: ping
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_ping_success(self, mock_redis):
        """Test pinging Redis server."""
        mock_redis.ping = AsyncMock(return_value=True)
        mock_redis.info = AsyncMock(return_value={"redis_version": "7.0.0"})

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import ping
            result = await ping()

        assert "PONG" in result
        assert "7.0.0" in result

    # ------------------------------------------------------------------
    # Tool 11: set_expiry
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_set_expiry(self, mock_redis):
        """Test setting TTL on a key."""
        mock_redis.expire = AsyncMock(return_value=True)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import set_expiry
            result = await set_expiry("mykey", ttl=300)

        assert "Set TTL on key 'mykey' to 300s" in result

    @pytest.mark.asyncio
    async def test_remove_expiry(self, mock_redis):
        """Test removing TTL from a key."""
        mock_redis.persist = AsyncMock(return_value=True)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import set_expiry
            result = await set_expiry("mykey", ttl=-1)

        assert "Removed expiry" in result

    # ------------------------------------------------------------------
    # Tool 12: increment
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_increment(self, mock_redis):
        """Test incrementing a numeric key."""
        mock_redis.incrby = AsyncMock(return_value=11)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import increment
            result = await increment("counter", amount=10)

        assert "incremented by 10" in result
        assert "new value: 11" in result

    # ------------------------------------------------------------------
    # Tool 13: pop_from_list
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_pop_from_list_left(self, mock_redis):
        """Test popping from the left of a list."""
        mock_redis.lpop = AsyncMock(return_value="first")
        mock_redis.llen = AsyncMock(return_value=4)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import pop_from_list
            result = await pop_from_list("mylist", pop_left=True)

        assert "Popped 'first' from list 'mylist'" in result
        assert "remaining: 4" in result

    @pytest.mark.asyncio
    async def test_pop_from_list_empty(self, mock_redis):
        """Test popping from an empty list."""
        mock_redis.lpop = AsyncMock(return_value=None)

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import pop_from_list
            result = await pop_from_list("empty", pop_left=True)

        assert "empty or doesn't exist" in result

    # ------------------------------------------------------------------
    # Tool 14: get_memory_info
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_memory_info(self, mock_redis):
        """Test getting Redis memory usage information."""
        mock_redis.info = AsyncMock(return_value={
            "used_memory_human": "2.50M",
            "used_memory_peak_human": "5.00M",
            "mem_fragmentation_ratio": 1.5
        })

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_memory_info
            result = await get_memory_info()

        assert "Redis Memory Usage" in result
        assert "Used: 2.50M" in result
        assert "Peak: 5.00M" in result

    # ------------------------------------------------------------------
    # Error handling tests
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_error_handling_network_failure(self, mock_redis):
        """Test error handling when network fails."""
        mock_redis.get = AsyncMock(side_effect=ConnectionError("Network error"))
        mock_redis.type = AsyncMock(side_effect=ConnectionError("Network error"))

        with patch('server.get_redis', AsyncMock(return_value=mock_redis)):
            from server import get_key
            result = await get_key("mykey")

        assert "Error" in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
