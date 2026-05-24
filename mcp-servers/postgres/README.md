# PostgreSQL MCP Server

A Model Context Protocol (MCP) server for PostgreSQL database operations with SQLite offline fallback support.

## Features

- **11 Tools** for comprehensive database management
- **Connection Pooling** with asyncpg for high performance
- **SSL/TLS Support** with configurable verification modes
- **SQLite Fallback** when PostgreSQL is unavailable
- **Parameterized Queries** to prevent SQL injection
- **Structured Logging** with detailed error messages

## Installation

```bash
pip install -r requirements.txt
```

## Configuration

Set these environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `DATABASE_URL` | PostgreSQL connection string | `postgresql://localhost/carpai` |
| `PG_SSL_MODE` | SSL mode: disable/allow/prefer/require/verify-ca/verify-full | `prefer` |
| `PG_SSL_ROOT_CERT` | Path to CA certificate file | _(empty)_ |
| `PG_OFFLINE_FALLBACK` | Enable SQLite fallback when PG unavailable | `0` |
| `MCP_DB_PATH` | Path for SQLite fallback database | `./carpai_mcp.db` |

### Example DATABASE_URL Formats

```bash
# Standard format
export DATABASE_URL="postgresql://user:password@localhost:5432/dbname"

# With SSL
export DATABASE_URL="postgresql://user:password@localhost:5432/dbname?sslmode=require"

# Unix socket
export DATABASE_URL="postgresql:///dbname?host=/var/run/postgresql"
```

## Available Tools

### 1. `status()`
Check database connection status and backend information.

**Example:**
```python
result = await status()
# Returns: "Database: Connected\nBackend: PostgreSQL\n..."
```

### 2. `execute_query(sql: str, params: list = None)`
Execute a SELECT query and return results as formatted table.

**Parameters:**
- `sql`: SQL query (use `$1`, `$2`... for parameters)
- `params`: Optional list of parameter values

**Example:**
```python
result = await execute_query(
    "SELECT * FROM users WHERE age > $1",
    params=[25]
)
```

### 3. `execute_write(sql: str, params: list = None)`
Execute write queries (INSERT/UPDATE/DELETE/CREATE/ALTER/DROP).

**Example:**
```python
result = await execute_write(
    "INSERT INTO users (name, email) VALUES ($1, $2)",
    params=["Alice", "alice@example.com"]
)
```

### 4. `list_tables(schema: str = "public")`
List all tables in the specified schema with sizes.

**Example:**
```python
result = await list_tables(schema="public")
# Returns: "Tables in schema 'public':\n  - users (8 kB)\n  - posts (16 kB)"
```

### 5. `describe_table(table_name: str, schema: str = "public")`
Describe a table's structure including columns, types, and constraints.

**Example:**
```python
result = await describe_table("users")
# Returns column definitions with types, nullability, defaults, constraints
```

### 6. `explain_query(sql: str, analyze: bool = False)`
Explain a SQL query execution plan for performance analysis.

**Parameters:**
- `sql`: Query to explain
- `analyze`: If true, runs the query to collect timing stats

**Example:**
```python
result = await explain_query("SELECT * FROM users WHERE email LIKE '%@example.com'")
```

### 7. `get_database_info()`
Get database metadata including version, size, and configuration.

**Example:**
```python
result = await get_database_info()
# Returns: "Database Information (PostgreSQL):\n  Name: carpai\n  User: admin\n..."
```

### 8. `get_indexes(table_name: str, schema: str = "public")`
List indexes on a table with definitions and sizes.

**Example:**
```python
result = await get_indexes("users")
```

### 9. `get_foreign_keys(table_name: str, schema: str = "public")`
List foreign key constraints on a table.

**Example:**
```python
result = await get_foreign_keys("posts")
```

### 10. `get_row_count(table_name: str, schema: str = "public")`
Get exact row count for a table.

**Example:**
```python
result = await get_row_count("users")
# Returns: "Table 'users' has 1,234 rows."
```

### 11. `backup_database(schema: str = "public")`
Generate a schema-only backup as CREATE TABLE statements.

**Example:**
```python
result = await backup_database()
# Returns SQL DDL statements for all tables
```

## Usage

### Start the Server

```bash
cd mcp-servers/postgres
python src/server.py
```

### Test the Server

```bash
pytest tests/test_postgres_mcp.py -v
```

## Security

- **SQL Injection Prevention**: All user inputs use parameterized queries
- **SSL/TLS Encryption**: Supports full certificate verification
- **Connection Pooling**: Limits concurrent connections (max 5 by default)
- **Error Handling**: Sensitive information is not leaked in error messages

## Testing

Run the test suite:

```bash
pip install pytest pytest-asyncio
pytest tests/test_postgres_mcp.py -v --cov=src
```

Test coverage includes:
- All 11 tools with mock databases
- Error handling scenarios
- Parameterized query safety
- SQLite fallback mode

## Troubleshooting

### Connection Issues

```bash
# Test PostgreSQL connectivity
psql postgresql://user:pass@localhost/dbname

# Check SSL configuration
export PG_SSL_MODE=require
python -c "import asyncpg; import asyncio; asyncio.run(asyncpg.connect('postgresql://...'))"
```

### Enable Debug Logging

```bash
export PYTHONDEBUG=1
export LOGLEVEL=DEBUG
python src/server.py
```

## License

MIT
