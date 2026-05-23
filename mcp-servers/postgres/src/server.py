"""
PostgreSQL MCP Server
=====================
Provides: SQL queries, Schema inspection, Table management, EXPLAIN analysis, Backup
via the Model Context Protocol (MCP).

Features:
- Connection pooling with asyncpg
- SSL/TLS support for secure connections
- SQLite offline fallback when PostgreSQL is unavailable
- Parameterized queries for SQL injection safety

Environment:
  DATABASE_URL        - PostgreSQL connection string (e.g. postgresql://user:pass@localhost/dbname)
  PG_SSL_MODE         - SSL mode: disable, allow, prefer, require, verify-ca, verify-full (default: prefer)
  PG_SSL_ROOT_CERT    - Path to CA certificate file (required for verify-ca and verify-full)
  PG_OFFLINE_FALLBACK - Set to "1" to enable SQLite fallback when PostgreSQL is unreachable
  MCP_DB_PATH         - Path for SQLite fallback database file (default: ./carpai_mcp.db)
"""

from mcp.server import FastMCP
import os
import sys
import json
import logging
import ssl as ssl_module
from typing import Optional, Any
from contextlib import asynccontextmanager

# Configure logging
logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(name)s: %(message)s")
logger = logging.getLogger("mcp-postgres")

mcp = FastMCP("postgres")

# ---------------------------------------------------------------------------
# Configuration from environment
# ---------------------------------------------------------------------------
DATABASE_URL = os.getenv("DATABASE_URL", "postgresql://localhost/carpai")
PG_SSL_MODE = os.getenv("PG_SSL_MODE", "prefer").lower()
PG_SSL_ROOT_CERT = os.getenv("PG_SSL_ROOT_CERT", "")
PG_OFFLINE_FALLBACK = os.getenv("PG_OFFLINE_FALLBACK", "0") == "1"
MCP_DB_PATH = os.getenv("MCP_DB_PATH", os.path.join(os.path.dirname(os.path.abspath(__file__)), "carpai_mcp.db"))

# ---------------------------------------------------------------------------
# Backend selection
# ---------------------------------------------------------------------------
BACKEND = "postgresql"  # or "sqlite"

async def _test_postgres_connection() -> bool:
    """Test if PostgreSQL is reachable. Returns True if connection succeeds."""
    try:
        import asyncpg
        conn = await asyncpg.connect(
            dsn=DATABASE_URL,
            timeout=5,
            ssl=_get_ssl_context(),
        )
        await conn.close()
        return True
    except Exception as e:
        logger.warning(f"PostgreSQL connection test failed: {e}")
        return False


def _get_ssl_context():
    """Build SSL context based on PG_SSL_MODE environment variable."""
    if PG_SSL_MODE == "disable":
        return False  # No SSL

    if PG_SSL_MODE in ("allow", "prefer"):
        return True  # Try SSL, fall back to no SSL

    ctx = ssl_module.create_default_context()

    if PG_SSL_MODE == "require":
        ctx.check_hostname = False
        ctx.verify_mode = ssl_module.CERT_NONE
    elif PG_SSL_MODE == "verify-ca":
        ctx.check_hostname = False
        ctx.verify_mode = ssl_module.CERT_REQUIRED
        if PG_SSL_ROOT_CERT:
            ctx.load_verify_locations(PG_SSL_ROOT_CERT)
    elif PG_SSL_MODE == "verify-full":
        ctx.check_hostname = True
        ctx.verify_mode = ssl_module.CERT_REQUIRED
        if PG_SSL_ROOT_CERT:
            ctx.load_verify_locations(PG_SSL_ROOT_CERT)
    else:
        logger.warning(f"Unknown PG_SSL_MODE '{PG_SSL_MODE}', defaulting to 'prefer'")
        return True

    return ctx


def _parse_dsn(dsn: str) -> dict:
    """Parse a PostgreSQL DSN to extract connection parameters for asyncpg."""
    import urllib.parse
    result = {"ssl": _get_ssl_context()}
    if dsn.startswith("postgresql://") or dsn.startswith("postgres://"):
        parsed = urllib.parse.urlparse(dsn)
        result["host"] = parsed.hostname or "localhost"
        result["port"] = parsed.port or 5432
        if parsed.username:
            result["user"] = urllib.parse.unquote(parsed.username)
        if parsed.password:
            result["password"] = urllib.parse.unquote(parsed.password)
        result["database"] = parsed.path.lstrip("/") if parsed.path else "carpai"
    else:
        # Simple key=value format (e.g., host=... dbname=...)
        for part in dsn.split():
            if "=" in part:
                key, value = part.split("=", 1)
                result[key.strip()] = value.strip()
    return result


# ---------------------------------------------------------------------------
# Connection pool (lazy initialization, supports fallback)
# ---------------------------------------------------------------------------
_pg_pool = None
_sqlite_conn = None


async def _init_pg_pool():
    """Initialize PostgreSQL connection pool with SSL support."""
    global _pg_pool, BACKEND
    try:
        import asyncpg
        conn_params = _parse_dsn(DATABASE_URL)
        logger.info(f"Connecting to PostgreSQL at {conn_params.get('host', 'unknown')}:{conn_params.get('port', 'unknown')} "
                     f"SSL mode: {PG_SSL_MODE}")
        _pg_pool = await asyncpg.create_pool(
            **conn_params,
            min_size=1,
            max_size=5,
            command_timeout=30,
            server_settings={"application_name": "CarpAI-MCP"},
        )
        BACKEND = "postgresql"
        logger.info("PostgreSQL connection pool established.")
    except Exception as e:
        if PG_OFFLINE_FALLBACK:
            logger.warning(f"Cannot connect to PostgreSQL: {e}")
            logger.info("Falling back to SQLite offline mode.")
            BACKEND = "sqlite"
            await _init_sqlite()
        else:
            logger.error(f"PostgreSQL connection failed: {e}")
            raise


async def _init_sqlite():
    """Initialize SQLite database for offline fallback mode."""
    import aiosqlite
    global _sqlite_conn
    db_dir = os.path.dirname(MCP_DB_PATH)
    if db_dir and not os.path.exists(db_dir):
        os.makedirs(db_dir, exist_ok=True)
    _sqlite_conn = await aiosqlite.connect(MCP_DB_PATH)
    _sqlite_conn.row_factory = aiosqlite.Row
    logger.info(f"SQLite fallback database opened at: {MCP_DB_PATH}")


@asynccontextmanager
async def get_connection():
    """Get a database connection (PostgreSQL pool or SQLite fallback)."""
    global _pg_pool, BACKEND

    if BACKEND == "postgresql":
        if _pg_pool is None:
            await _init_pg_pool()
        async with _pg_pool.acquire() as conn:
            yield conn
    else:
        if _sqlite_conn is None:
            await _init_sqlite()
        yield _sqlite_conn


# ---------------------------------------------------------------------------
# General helpers
# ---------------------------------------------------------------------------

def _format_rows_sqlite(rows, max_rows=100) -> str:
    """Format aiosqlite rows as tabular text."""
    if not rows:
        return "Query returned no rows."
    columns = rows[0].keys() if hasattr(rows[0], 'keys') else [f"col{i}" for i in range(len(rows[0]))]
    columns = list(columns)
    header = " | ".join(columns)
    separator = "-+-".join("-" * min(len(c), 50) for c in columns)
    lines = [header, separator]
    for row in rows[:max_rows]:
        values = []
        for c in columns:
            val = row[c] if isinstance(row, dict) else row[columns.index(c)] if isinstance(row, (list, tuple)) else "?"
            if val is None:
                values.append("NULL")
            elif isinstance(val, (dict, list)):
                values.append(json.dumps(val, ensure_ascii=False)[:50])
            else:
                values.append(str(val)[:50])
        lines.append(" | ".join(values))
    if len(rows) > max_rows:
        lines.append(f"... and {len(rows) - max_rows} more rows (truncated)")
    return "\n".join(lines)


def _format_rows_pg(rows, max_rows=100) -> str:
    """Format asyncpg rows as tabular text."""
    if not rows:
        return "Query returned no rows."
    columns = list(rows[0].keys())
    header = " | ".join(columns)
    separator = "-+-".join("-" * min(len(c), 50) for c in columns)
    lines = [header, separator]
    for row in rows[:max_rows]:
        values = []
        for c in columns:
            val = row[c]
            if val is None:
                values.append("NULL")
            elif isinstance(val, (dict, list)):
                values.append(json.dumps(val, ensure_ascii=False)[:50])
            else:
                values.append(str(val)[:50])
        lines.append(" | ".join(values))
    if len(rows) > max_rows:
        lines.append(f"... and {len(rows) - max_rows} more rows (truncated)")
    return "\n".join(lines)


def _format_rows(rows, max_rows=100) -> str:
    """Format query results as tabular text (auto-detect backend)."""
    if BACKEND == "sqlite":
        return _format_rows_sqlite(rows, max_rows)
    return _format_rows_pg(rows, max_rows)


async def _fetch_all(sql: str, *params):
    """Execute a SELECT query and return all rows."""
    async with get_connection() as conn:
        if BACKEND == "sqlite":
            cursor = await conn.execute(sql, params or None)
            return await cursor.fetchall()
        else:
            if params:
                return await conn.fetch(sql, *params)
            return await conn.fetch(sql)


async def _execute(sql: str, *params):
    """Execute a write query (INSERT/UPDATE/DELETE/CREATE)."""
    async with get_connection() as conn:
        if BACKEND == "sqlite":
            await conn.execute(sql, params or None)
            await conn.commit()
        else:
            if params:
                await conn.execute(sql, *params)
            else:
                await conn.execute(sql)


# ---------------------------------------------------------------------------
# Tool 1: status - check database connection
# ---------------------------------------------------------------------------
@mcp.tool()
async def status() -> str:
    """Check database connection status and backend information."""
    try:
        if BACKEND == "postgresql" and _pg_pool:
            async with _pg_pool.acquire() as conn:
                version = await conn.fetchval("SELECT version()")
                db_name = await conn.fetchval("SELECT current_database()")
                return f"""Database: Connected
Backend: PostgreSQL
Database: {db_name}
Version: {version[:80]}
SSL: {PG_SSL_MODE}
Pool: {_pg_pool.get_size()} connections"""
        elif BACKEND == "sqlite" and _sqlite_conn:
            import sqlite3
            ver = sqlite3.sqlite_version
            return f"""Database: Connected
Backend: SQLite (offline fallback)
Version: {ver}
File: {MCP_DB_PATH}
SSL: N/A (SQLite)"""
        else:
            return "Database: Not connected. Check configuration."
    except Exception as e:
        return f"Database: Error - {e}"


# ---------------------------------------------------------------------------
# Tool 2: execute_query
# ---------------------------------------------------------------------------
@mcp.tool()
async def execute_query(sql: str, params: list = None) -> str:
    """Execute a SELECT query and return results.
    
    Args:
        sql: SQL query (use $1, $2... for PostgreSQL or ? for SQLite)
        params: Optional list of parameter values for parameterized queries
    """
    if BACKEND == "sqlite":
        sql = sql.replace("$1", "?").replace("$2", "?").replace("$3", "?")

    try:
        if params:
            sql_upper = sql.strip().upper()
            if sql_upper.startswith("SELECT") or sql_upper.startswith("WITH") or sql_upper.startswith("PRAGMA"):
                rows = await _fetch_all(sql, *params)
            else:
                await _execute(sql, *params)
                return f"Query executed successfully ({params})"
        else:
            sql_upper = sql.strip().upper()
            if sql_upper.startswith("SELECT") or sql_upper.startswith("WITH") or sql_upper.startswith("PRAGMA"):
                rows = await _fetch_all(sql)
            else:
                await _execute(sql)
                return "Query executed successfully."

        return _format_rows(rows)
    except Exception as e:
        logger.error(f"Query error: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# Tool 3: execute_write
# ---------------------------------------------------------------------------
@mcp.tool()
async def execute_write(sql: str, params: list = None) -> str:
    """Execute a write query (INSERT/UPDATE/DELETE/CREATE/ALTER/DROP/TRUNCATE).
    
    Args:
        sql: SQL write query (use $1, $2... for PostgreSQL or ? for SQLite)
        params: Optional list of parameter values
    """
    try:
        if BACKEND == "sqlite":
            sql = sql.replace("$1", "?").replace("$2", "?").replace("$3", "?")

        if params:
            await _execute(sql, *params)
        else:
            await _execute(sql)
        return "Write query executed successfully."
    except Exception as e:
        logger.error(f"Write query error: {e}")
        return f"Error: {e}"


# ---------------------------------------------------------------------------
# Tool 4: list_tables
# ---------------------------------------------------------------------------
@mcp.tool()
async def list_tables(schema: str = "public") -> str:
    """List all tables in the specified schema (PostgreSQL) or all tables (SQLite).
    
    Args:
        schema: Schema name (default: public, only used for PostgreSQL)
    """
    try:
        if BACKEND == "sqlite":
            rows = await _fetch_all(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"
            )
            if not rows:
                return "No tables found."
            lines = ["Tables (SQLite):", ""]
            for row in rows:
                name = row["name"] if hasattr(row, 'keys') else row[0]
                lines.append(f"  - {name}")
            return "\n".join(lines)
        else:
            rows = await _fetch_all(
                "SELECT table_name, table_type, "
                "pg_size_pretty(pg_total_relation_size(quote_ident(table_schema) || '.' || quote_ident(table_name))) as size "
                "FROM information_schema.tables "
                "WHERE table_schema = $1 AND table_type = 'BASE TABLE' "
                "ORDER BY table_name",
                schema,
            )
            if not rows:
                return f"No tables found in schema '{schema}'."
            lines = [f"Tables in schema '{schema}':", ""]
            for row in rows:
                lines.append(f"  - {row['table_name']:30s} ({row['size']})")
            return "\n".join(lines)
    except Exception as e:
        return f"Error listing tables: {e}"


# ---------------------------------------------------------------------------
# Tool 5: describe_table
# ---------------------------------------------------------------------------
@mcp.tool()
async def describe_table(table_name: str, schema: str = "public") -> str:
    """Describe a table's schema including columns, types, and constraints.
    
    Args:
        table_name: Name of the table
        schema: Schema name (default: public, only used for PostgreSQL)
    """
    try:
        if BACKEND == "sqlite":
            rows = await _fetch_all(f"PRAGMA table_info({table_name})")
            if not rows:
                return f"Table '{table_name}' not found."
            lines = [f"Table: {table_name}", ""]
            for row in rows:
                name = row["name"]
                col_type = row["type"] or "N/A"
                nullable = "NULL" if row["notnull"] == 0 else "NOT NULL"
                default = f" DEFAULT {row['dflt_value']}" if row["dflt_value"] else ""
                pk = " [PRIMARY KEY]" if row["pk"] else ""
                lines.append(f"  - {name:25s} {col_type:15s} {nullable}{default}{pk}")
            return "\n".join(lines)
        else:
            rows = await _fetch_all(
                "SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, "
                "  t.constraint_type "
                "FROM information_schema.columns c "
                "LEFT JOIN information_schema.key_column_usage k "
                "  ON c.table_schema = k.table_schema AND c.table_name = k.table_name "
                "  AND c.column_name = k.column_name "
                "LEFT JOIN information_schema.table_constraints t "
                "  ON k.constraint_schema = t.constraint_schema "
                "  AND k.constraint_name = t.constraint_name "
                "WHERE c.table_schema = $1 AND c.table_name = $2 "
                "ORDER BY c.ordinal_position",
                schema, table_name,
            )
            if not rows:
                return f"Table '{schema}.{table_name}' not found."
            count_row = await _fetch_all(
                "SELECT reltuples::bigint AS cnt FROM pg_class WHERE oid = "
                "(quote_ident($1) || '.' || quote_ident($2))::regclass::oid",
                schema, table_name,
            )
            est_rows = count_row[0]["cnt"] if count_row else "N/A"
            lines = [f"Table: {schema}.{table_name}", f"Estimated rows: {est_rows}", "", "Columns:"]
            for col in rows:
                col_type = col["data_type"]
                nullable = "NULL" if col["is_nullable"] == "YES" else "NOT NULL"
                default = f" DEFAULT {col['column_default']}" if col["column_default"] else ""
                constraint = f" [{col['constraint_type']}]" if col["constraint_type"] else ""
                lines.append(f"  - {col['column_name']:25s} {col_type:15s} {nullable}{default}{constraint}")
            return "\n".join(lines)
    except Exception as e:
        return f"Error describing table: {e}"


# ---------------------------------------------------------------------------
# Tool 6: explain_query
# ---------------------------------------------------------------------------
@mcp.tool()
async def explain_query(sql: str, analyze: bool = False) -> str:
    """Explain a SQL query execution plan.
    
    Args:
        sql: SQL query to explain
        analyze: Whether to collect timing stats (PostgreSQL only, runs the query!)
    """
    if BACKEND == "sqlite":
        return "EXPLAIN is not supported in SQLite offline mode."
    try:
        if analyze:
            rows = await _fetch_all(f"EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) {sql}")
        else:
            rows = await _fetch_all(f"EXPLAIN (FORMAT TEXT) {sql}")
        return "\n".join(r["QUERY PLAN"] for r in rows)
    except Exception as e:
        return f"Error explaining query: {e}"


# ---------------------------------------------------------------------------
# Tool 7: get_database_info
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_database_info() -> str:
    """Get database metadata including version, size, and configuration."""
    try:
        if BACKEND == "sqlite":
            import sqlite3
            rows = await _fetch_all("SELECT sqlite_version() AS ver")
            ver = rows[0]["ver"] if rows else sqlite3.sqlite_version
            size = os.path.getsize(MCP_DB_PATH) if os.path.exists(MCP_DB_PATH) else 0
            size_str = f"{size / 1024:.1f} KB" if size < 1024 * 1024 else f"{size / (1024*1024):.1f} MB"
            table_count = len(await _fetch_all("SELECT name FROM sqlite_master WHERE type='table'"))
            return f"""Database Information (SQLite):
  File: {MCP_DB_PATH}
  Version: {ver}
  Size: {size_str}
  Tables: {table_count}
  Backend: SQLite (offline fallback)"""
        else:
            async with get_connection() as conn:
                version = await conn.fetchval("SELECT version()")
                db_name = await conn.fetchval("SELECT current_database()")
                user = await conn.fetchval("SELECT current_user")
                size = await conn.fetchval(
                    "SELECT pg_size_pretty(pg_database_size($1))", db_name
                )
                return f"""Database Information (PostgreSQL):
  Name: {db_name}
  User: {user}
  Version: {version[:80]}
  Size: {size}
  SSL: {PG_SSL_MODE}
  Backend: PostgreSQL"""
    except Exception as e:
        return f"Error getting database info: {e}"


# ---------------------------------------------------------------------------
# Tool 8: get_indexes
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_indexes(table_name: str, schema: str = "public") -> str:
    """List indexes on a table.
    
    Args:
        table_name: Name of the table
        schema: Schema name (default: public, PostgreSQL only)
    """
    try:
        if BACKEND == "sqlite":
            rows = await _fetch_all(f"PRAGMA index_list({table_name})")
            if not rows:
                return f"No indexes on '{table_name}'."
            lines = [f"Indexes on {table_name}:", ""]
            for row in rows:
                idx_name = row["name"]
                unique = "UNIQUE" if row["unique"] else ""
                detail = await _fetch_all(f"PRAGMA index_info({idx_name})")
                cols = ", ".join(d["name"] for d in detail)
                lines.append(f"  - {idx_name} ({unique}) ON {cols}")
            return "\n".join(lines)
        else:
            rows = await _fetch_all(
                "SELECT i.indexname, i.indexdef, "
                "pg_size_pretty(pg_relation_size(i.indexname::regclass)) as size "
                "FROM pg_indexes i "
                "WHERE i.schemaname = $1 AND i.tablename = $2 "
                "ORDER BY i.indexname",
                schema, table_name,
            )
            if not rows:
                return f"No indexes on '{schema}.{table_name}'."
            lines = [f"Indexes on {schema}.{table_name}:", ""]
            for row in rows:
                lines.append(f"  - {row['indexname']:30s} ({row['size']})")
                lines.append(f"    {row['indexdef']}")
                lines.append("")
            return "\n".join(lines)
    except Exception as e:
        return f"Error getting indexes: {e}"


# ---------------------------------------------------------------------------
# Tool 9: get_foreign_keys
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_foreign_keys(table_name: str, schema: str = "public") -> str:
    """List foreign key constraints on a table.
    
    Args:
        table_name: Name of the table
        schema: Schema name (default: public, PostgreSQL only)
    """
    try:
        if BACKEND == "sqlite":
            rows = await _fetch_all(f"PRAGMA foreign_key_list({table_name})")
            if not rows:
                return f"No foreign keys on '{table_name}'."
            lines = [f"Foreign keys on {table_name}:", ""]
            for row in rows:
                lines.append(f"  - {row['from']} -> {row['table']}.{row['to']} (on {row['on_update']}/{row['on_delete']})")
            return "\n".join(lines)
        else:
            rows = await _fetch_all(
                "SELECT kcu.constraint_name, kcu.column_name, "
                "  ccu.table_schema AS foreign_table_schema, "
                "  ccu.table_name AS foreign_table_name, "
                "  ccu.column_name AS foreign_column_name "
                "FROM information_schema.table_constraints AS tc "
                "JOIN information_schema.key_column_usage AS kcu "
                "  ON tc.constraint_name = kcu.constraint_name "
                "  AND tc.table_schema = kcu.table_schema "
                "JOIN information_schema.constraint_column_usage AS ccu "
                "  ON ccu.constraint_name = tc.constraint_name "
                "  AND ccu.table_schema = tc.table_schema "
                "WHERE tc.constraint_type = 'FOREIGN KEY' "
                "  AND tc.table_schema = $1 AND tc.table_name = $2",
                schema, table_name,
            )
            if not rows:
                return f"No foreign keys on '{schema}.{table_name}'."
            lines = [f"Foreign keys on {schema}.{table_name}:", ""]
            for row in rows:
                lines.append(f"  - {row['constraint_name']}: {row['column_name']} -> "
                             f"{row['foreign_table_schema']}.{row['foreign_table_name']}.{row['foreign_column_name']}")
            return "\n".join(lines)
    except Exception as e:
        return f"Error getting foreign keys: {e}"


# ---------------------------------------------------------------------------
# Tool 10: get_row_count
# ---------------------------------------------------------------------------
@mcp.tool()
async def get_row_count(table_name: str, schema: str = "public") -> str:
    """Get exact row count for a table.
    
    Args:
        table_name: Name of the table
        schema: Schema name (default: public, PostgreSQL only)
    """
    try:
        if BACKEND == "sqlite":
            rows = await _fetch_all(f"SELECT COUNT(*) AS cnt FROM [{table_name}]")
        else:
            rows = await _fetch_all(f"SELECT COUNT(*) AS cnt FROM {schema}.{table_name}")
        count = rows[0]["cnt"] if rows else 0
        return f"Table '{table_name}' has {count:,} rows."
    except Exception as e:
        return f"Error getting row count: {e}"


# ---------------------------------------------------------------------------
# Tool 11: backup_database (schema only)
# ---------------------------------------------------------------------------
@mcp.tool()
async def backup_database(schema: str = "public") -> str:
    """Generate a schema-only backup (CREATE TABLE statements).
    
    Args:
        schema: Schema to backup (default: public, PostgreSQL only)
    """
    try:
        from datetime import datetime

        if BACKEND == "sqlite":
            rows = await _fetch_all("SELECT sql FROM sqlite_master WHERE type='table' AND sql IS NOT NULL")
            lines = [f"-- CarpAI SQLite Backup", f"-- Generated: {datetime.now()}", ""]
            for row in rows:
                lines.append(row["sql"] + ";")
            return "\n".join(lines)

        tables = await _fetch_all(
            "SELECT table_name FROM information_schema.tables "
            "WHERE table_schema = $1 AND table_type = 'BASE TABLE'",
            schema,
        )
        if not tables:
            return f"No tables found in schema '{schema}'."

        lines = [f"-- CarpAI Backup of schema '{schema}'", f"-- Generated: {datetime.now()}", ""]
        for tbl in tables:
            cols = await _fetch_all(
                "SELECT column_name, data_type, character_maximum_length, "
                "  is_nullable, column_default "
                "FROM information_schema.columns "
                "WHERE table_schema = $1 AND table_name = $2 "
                "ORDER BY ordinal_position",
                schema, tbl["table_name"],
            )
            lines.append(f"CREATE TABLE {schema}.{tbl['table_name']} (")
            col_lines = []
            for c in cols:
                col_def = f"  {c['column_name']} {c['data_type']}"
                if c["character_maximum_length"]:
                    col_def += f"({c['character_maximum_length']})"
                if c["is_nullable"] == "NO":
                    col_def += " NOT NULL"
                if c["column_default"]:
                    col_def += f" DEFAULT {c['column_default']}"
                col_lines.append(col_def)
            lines.append(",\n".join(col_lines))
            lines.append(");\n")
        return "\n".join(lines)
    except Exception as e:
        return f"Error generating backup: {e}"


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
async def main():
    """Initialize backend and start MCP server."""
    logger.info("Starting PostgreSQL MCP Server...")
    logger.info(f"  Database URL: {DATABASE_URL}")
    logger.info(f"  SSL mode: {PG_SSL_MODE}")
    logger.info(f"  Offline fallback: {'enabled (SQLite)' if PG_OFFLINE_FALLBACK else 'disabled'}")

    global BACKEND
    try:
        import asyncpg
    except ImportError:
        logger.warning("asyncpg not installed, using SQLite offline mode.")
        BACKEND = "sqlite"
        await _init_sqlite()
        mcp.run()
        return

    # Try PostgreSQL first, fallback to SQLite if configured
    if await _test_postgres_connection():
        await _init_pg_pool()
        logger.info("PostgreSQL connected. Starting MCP server...")
    elif PG_OFFLINE_FALLBACK:
        logger.info("PostgreSQL unavailable. Falling back to SQLite.")
        BACKEND = "sqlite"
        await _init_sqlite()
    else:
        logger.warning(
            "PostgreSQL not available and offline fallback is disabled.\n"
            "  Set PG_OFFLINE_FALLBACK=1 to enable SQLite fallback, or\n"
            "  Configure DATABASE_URL for PostgreSQL, or\n"
            "  Install asyncpg: pip install asyncpg"
        )
        BACKEND = "sqlite"
        await _init_sqlite()

    mcp.run()


if __name__ == "__main__":
    import asyncio
    asyncio.run(main())
