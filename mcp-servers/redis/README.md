# Redis MCP Server

A Model Context Protocol (MCP) server for Redis operations with comprehensive data structure support.

## Features

- **14 Tools** for complete Redis management
- **Connection Pooling** with retry and keepalive support
- **Full Data Structure Support**: Strings, Lists, Sets, Hashes, Sorted Sets
- **TTL Management**: Set, get, and remove key expiration
- **Memory Monitoring**: Track Redis memory usage
- **Error Handling**: Comprehensive error recovery and logging

## Installation

```bash
pip install -r requirements.txt
```

## Configuration

Set these environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `REDIS_URL` | Redis connection string | `redis://localhost:6379` |

### Example REDIS_URL Formats

```bash
# Local Redis
export REDIS_URL="redis://localhost:6379"

# With password
export REDIS_URL="redis://:password@localhost:6379"

# With database number
export REDIS_URL="redis://localhost:6379/0"

# Remote with SSL
export REDIS_URL="rediss://:password@redis.example.com:6380/0"
```

## Available Tools

### 1. `get_key(key: str)`
Get the value of a key, auto-detecting its type (string/list/set/hash/zset).

**Example:**
```python
result = await get_key("user:123")
# Returns formatted output based on key type
```

### 2. `set_key(key: str, value: str, ttl: int = 0)`
Set a string value with optional TTL.

**Parameters:**
- `key`: Redis key name
- `value`: String value to store
- `ttl`: Time-to-live in seconds (0 = no expiry)

**Example:**
```python
result = await set_key("session:abc", "active", ttl=3600)
```

### 3. `delete_key(key: str)`
Delete a key from Redis.

**Example:**
```python
result = await delete_key("old_key")
```

### 4. `list_keys(pattern: str = "*")`
List keys matching a pattern with their types.

**Example:**
```python
result = await list_keys(pattern="user:*")
# Returns: "Keys matching 'user:*' (5 total):\n  - user:1 (string)\n..."
```

### 5. `get_ttl(key: str)`
Get the TTL of a key.

**Returns:**
- `-2`: Key does not exist
- `-1`: Key has no expiry (persistent)
- `>0`: Remaining TTL in seconds

**Example:**
```python
result = await get_ttl("session:abc")
```

### 6. `flush_db(confirm: bool = False)`
Flush all keys in the current database (requires confirmation).

**Example:**
```python
# First call returns warning
result = await flush_db(confirm=False)

# Second call executes flush
result = await flush_db(confirm=True)
```

### 7. `push_to_list(key: str, value: str, push_left: bool = False)`
Push a value to a Redis list (LPUSH or RPUSH).

**Example:**
```python
result = await push_to_list("queue", "task1", push_left=False)  # RPUSH
result = await push_to_list("stack", "item1", push_left=True)   # LPUSH
```

### 8. `add_to_set(key: str, member: str)`
Add a member to a Redis set.

**Example:**
```python
result = await add_to_set("tags", "python")
```

### 9. `hash_set(key: str, field: str, value: str)`
Set a field in a Redis hash.

**Example:**
```python
result = await hash_set("user:123", "name", "Alice")
```

### 10. `ping()`
Check if Redis server is responsive.

**Example:**
```python
result = await ping()
# Returns: "Redis PONG (version: 7.0.0)"
```

### 11. `set_expiry(key: str, ttl: int)`
Set or update the TTL on a key.

**Parameters:**
- `ttl`: Seconds (-1 to remove expiry)

**Example:**
```python
result = await set_expiry("session:abc", ttl=600)
result = await set_expiry("permanent_key", ttl=-1)  # Remove expiry
```

### 12. `increment(key: str, amount: int = 1)`
Increment a numeric key's value.

**Example:**
```python
result = await increment("counter", amount=5)
```

### 13. `pop_from_list(key: str, pop_left: bool = True)`
Pop an element from a Redis list (LPOP or RPOP).

**Example:**
```python
result = await pop_from_list("queue", pop_left=True)   # LPOP
result = await pop_from_list("queue", pop_left=False)  # RPOP
```

### 14. `get_memory_info()`
Get Redis memory usage information.

**Example:**
```python
result = await get_memory_info()
# Returns: "Redis Memory Usage:\n  Used: 2.50M\n  Peak: 5.00M\n..."
```

## Usage

### Start the Server

```bash
cd mcp-servers/redis
python src/server.py
```

### Test the Server

```bash
pytest tests/test_redis_mcp.py -v
```

## Security

- **Connection Pooling**: Limits concurrent connections (max 10 by default)
- **Retry on Timeout**: Automatic retry for transient failures
- **Socket Keepalive**: Detects dead connections early
- **No Command Injection**: All commands use redis-py's safe API

## Testing

Run the test suite:

```bash
pip install pytest pytest-asyncio
pytest tests/test_redis_mcp.py -v --cov=src
```

Test coverage includes:
- All 14 tools with mock Redis
- Error handling scenarios
- Different data types (string/list/set/hash/zset)
- TTL management edge cases

## Troubleshooting

### Connection Issues

```bash
# Test Redis connectivity
redis-cli ping

# Check Redis logs
tail -f /var/log/redis/redis-server.log
```

### Enable Debug Logging

```bash
export LOGLEVEL=DEBUG
python src/server.py
```

### Performance Tips

- Use connection pooling (enabled by default)
- Batch operations when possible
- Monitor memory usage with `get_memory_info()`
- Set appropriate TTLs to prevent memory leaks

## License

MIT
