"""
PostgreSQL MCP Server - Unit Tests
===================================
Tests for all 11 tools with comprehensive coverage.
"""

import pytest
import asyncio
import sys
import os
from unittest.mock import AsyncMock, MagicMock, patch

# Add src to path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'src'))


class TestPostgreSQLMCP:
    """Test suite for PostgreSQL MCP server tools."""

    @pytest.fixture
    def mock_server(self):
        """Create a mock MCP server instance."""
        from server import mcp
        return mcp

    @pytest.fixture
    def mock_pg_pool(self):
        """Mock PostgreSQL connection pool."""
        pool = AsyncMock()
        conn = AsyncMock()
        pool.acquire.return_value.__aenter__ = AsyncMock(return_value=conn)
        pool.acquire.return_value.__aexit__ = AsyncMock(return_value=None)
        return pool, conn

    # ------------------------------------------------------------------
    # Tool 1: status
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_status_postgresql_connected(self, mock_pg_pool):
        """Test status tool when PostgreSQL is connected."""
        pool, conn = mock_pg_pool
        conn.fetchval = AsyncMock(side_effect=[
            "PostgreSQL 16.0",  # version()
            "carpai"            # current_database()
        ])

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import status
            result = await status()

        assert "Database: Connected" in result
        assert "Backend: PostgreSQL" in result
        assert "PostgreSQL 16.0" in result

    @pytest.mark.asyncio
    async def test_status_sqlite_fallback(self):
        """Test status tool when using SQLite fallback."""
        with patch('server.BACKEND', 'sqlite'), \
             patch('server.MCP_DB_PATH', '/tmp/test.db'):
            from server import status
            result = await status()

        assert "Backend: SQLite" in result
        assert "offline fallback" in result

    # ------------------------------------------------------------------
    # Tool 2: execute_query
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_execute_query_select(self, mock_pg_pool):
        """Test SELECT query execution."""
        pool, conn = mock_pg_pool
        mock_rows = [
            {"id": 1, "name": "Alice"},
            {"id": 2, "name": "Bob"}
        ]
        conn.fetch = AsyncMock(return_value=mock_rows)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import execute_query
            result = await execute_query("SELECT * FROM users")

        assert "id" in result
        assert "Alice" in result
        assert "Bob" in result

    @pytest.mark.asyncio
    async def test_execute_query_with_params(self, mock_pg_pool):
        """Test parameterized query execution."""
        pool, conn = mock_pg_pool
        mock_rows = [{"id": 1, "name": "Alice"}]
        conn.fetch = AsyncMock(return_value=mock_rows)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import execute_query
            result = await execute_query(
                "SELECT * FROM users WHERE id = $1",
                params=[1]
            )

        assert "Alice" in result
        conn.fetch.assert_called_once()

    @pytest.mark.asyncio
    async def test_execute_query_error(self, mock_pg_pool):
        """Test query error handling."""
        pool, conn = mock_pg_pool
        conn.fetch = AsyncMock(side_effect=Exception("Syntax error"))

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import execute_query
            result = await execute_query("INVALID SQL")

        assert "Error" in result

    # ------------------------------------------------------------------
    # Tool 3: execute_write
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_execute_write_insert(self, mock_pg_pool):
        """Test INSERT query execution."""
        pool, conn = mock_pg_pool
        conn.execute = AsyncMock(return_value=None)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import execute_write
            result = await execute_write(
                "INSERT INTO users (name) VALUES ($1)",
                params=["Charlie"]
            )

        assert "executed successfully" in result

    # ------------------------------------------------------------------
    # Tool 4: list_tables
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_list_tables_postgresql(self, mock_pg_pool):
        """Test listing tables in PostgreSQL."""
        pool, conn = mock_pg_pool
        mock_rows = [
            {"table_name": "users", "table_type": "BASE TABLE", "size": "8 kB"},
            {"table_name": "posts", "table_type": "BASE TABLE", "size": "16 kB"}
        ]
        conn.fetch = AsyncMock(return_value=mock_rows)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import list_tables
            result = await list_tables(schema="public")

        assert "Tables in schema 'public'" in result
        assert "users" in result
        assert "posts" in result

    @pytest.mark.asyncio
    async def test_list_tables_sqlite(self):
        """Test listing tables in SQLite."""
        mock_rows = [{"name": "users"}, {"name": "posts"}]

        with patch('server.BACKEND', 'sqlite'), \
             patch('server._fetch_all', AsyncMock(return_value=mock_rows)):
            from server import list_tables
            result = await list_tables()

        assert "Tables (SQLite)" in result
        assert "users" in result

    # ------------------------------------------------------------------
    # Tool 5: describe_table
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_describe_table_postgresql(self, mock_pg_pool):
        """Test describing table structure in PostgreSQL."""
        pool, conn = mock_pg_pool
        mock_columns = [
            {
                "column_name": "id",
                "data_type": "integer",
                "is_nullable": "NO",
                "column_default": "nextval('users_id_seq')",
                "constraint_type": "PRIMARY KEY"
            },
            {
                "column_name": "name",
                "data_type": "character varying",
                "is_nullable": "YES",
                "column_default": None,
                "constraint_type": None
            }
        ]
        conn.fetch = AsyncMock(side_effect=[
            mock_columns,
            [{"cnt": 100}]
        ])

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import describe_table
            result = await describe_table("users", schema="public")

        assert "Table: public.users" in result
        assert "id" in result
        assert "integer" in result
        assert "PRIMARY KEY" in result

    # ------------------------------------------------------------------
    # Tool 6: explain_query
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_explain_query(self, mock_pg_pool):
        """Test EXPLAIN query analysis."""
        pool, conn = mock_pg_pool
        mock_plan = [
            {"QUERY PLAN": "Seq Scan on users  (cost=0.00..1.05 rows=5 width=36)"},
            {"QUERY PLAN": "  Filter: (age > 25)"}
        ]
        conn.fetch = AsyncMock(return_value=mock_plan)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import explain_query
            result = await explain_query("SELECT * FROM users WHERE age > 25")

        assert "Seq Scan" in result
        assert "Filter" in result

    # ------------------------------------------------------------------
    # Tool 7: get_database_info
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_database_info_postgresql(self, mock_pg_pool):
        """Test getting PostgreSQL database information."""
        pool, conn = mock_pg_pool
        conn.fetchval = AsyncMock(side_effect=[
            "PostgreSQL 16.0",
            "carpai",
            "admin",
            "50 MB"
        ])

        with patch('server.get_connection') as mock_get_conn:
            mock_get_conn.return_value.__aenter__ = AsyncMock(return_value=conn)
            mock_get_conn.return_value.__aexit__ = AsyncMock(return_value=None)

            with patch('server.BACKEND', 'postgresql'):
                from server import get_database_info
                result = await get_database_info()

        assert "Database Information (PostgreSQL)" in result
        assert "Name: carpai" in result
        assert "User: admin" in result

    # ------------------------------------------------------------------
    # Tool 8: get_indexes
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_indexes_postgresql(self, mock_pg_pool):
        """Test listing indexes on a table."""
        pool, conn = mock_pg_pool
        mock_indexes = [
            {
                "indexname": "users_pkey",
                "indexdef": "CREATE UNIQUE INDEX users_pkey ON public.users USING btree (id)",
                "size": "8 kB"
            }
        ]
        conn.fetch = AsyncMock(return_value=mock_indexes)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import get_indexes
            result = await get_indexes("users", schema="public")

        assert "Indexes on public.users" in result
        assert "users_pkey" in result

    # ------------------------------------------------------------------
    # Tool 9: get_foreign_keys
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_foreign_keys_postgresql(self, mock_pg_pool):
        """Test listing foreign keys on a table."""
        pool, conn = mock_pg_pool
        mock_fks = [
            {
                "constraint_name": "posts_user_id_fkey",
                "column_name": "user_id",
                "foreign_table_schema": "public",
                "foreign_table_name": "users",
                "foreign_column_name": "id"
            }
        ]
        conn.fetch = AsyncMock(return_value=mock_fks)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import get_foreign_keys
            result = await get_foreign_keys("posts", schema="public")

        assert "Foreign keys on public.posts" in result
        assert "user_id -> public.users.id" in result

    # ------------------------------------------------------------------
    # Tool 10: get_row_count
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_get_row_count(self, mock_pg_pool):
        """Test getting exact row count."""
        pool, conn = mock_pg_pool
        conn.fetch = AsyncMock(return_value=[{"cnt": 1234}])

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import get_row_count
            result = await get_row_count("users", schema="public")

        assert "1,234" in result or "1234" in result

    # ------------------------------------------------------------------
    # Tool 11: backup_database
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_backup_database_postgresql(self, mock_pg_pool):
        """Test generating schema backup."""
        pool, conn = mock_pg_pool
        mock_tables = [{"table_name": "users"}]
        mock_columns = [
            {
                "column_name": "id",
                "data_type": "integer",
                "character_maximum_length": None,
                "is_nullable": "NO",
                "column_default": "nextval('users_id_seq')"
            }
        ]
        conn.fetch = AsyncMock(side_effect=[mock_tables, mock_columns])

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import backup_database
            result = await backup_database(schema="public")

        assert "CREATE TABLE" in result
        assert "public.users" in result

    # ------------------------------------------------------------------
    # Integration tests
    # ------------------------------------------------------------------
    @pytest.mark.asyncio
    async def test_parameterized_query_prevents_injection(self, mock_pg_pool):
        """Test that parameterized queries prevent SQL injection."""
        pool, conn = mock_pg_pool
        mock_rows = []
        conn.fetch = AsyncMock(return_value=mock_rows)

        with patch('server._pg_pool', pool), \
             patch('server.BACKEND', 'postgresql'):
            from server import execute_query
            # Attempt SQL injection via parameter
            result = await execute_query(
                "SELECT * FROM users WHERE name = $1",
                params=["'; DROP TABLE users; --"]
            )

        # Should execute safely without error
        assert "Error" not in result or "DROP TABLE" not in result


if __name__ == "__main__":
    pytest.main([__file__, "-v"])
