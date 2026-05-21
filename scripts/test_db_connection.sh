#!/bin/bash
# Test database connections after setup
# Usage: ./scripts/test_db_connection.sh

set -e

echo "🧪 Testing Database Connections..."
echo ""

# Test PostgreSQL
echo "1️⃣  Testing PostgreSQL connection..."
if docker exec carpai-postgres pg_isready -U carpai -d carpai &> /dev/null; then
    echo "   ✅ PostgreSQL is ready"

    # Check if migrations were applied
    table_count=$(docker exec carpai-postgres psql -U carpai -d carpai -t -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public';" 2>/dev/null | tr -d ' ')

    if [ "$table_count" -gt "0" ]; then
        echo "   ✅ Database tables created: $table_count tables"

        # List tables
        echo ""
        echo "   📋 Database tables:"
        docker exec carpai-postgres psql -U carpai -d carpai -c "\dt" 2>/dev/null | grep -E "^ public" || true
    else
        echo "   ⚠️  No tables found. Migrations may not have run."
    fi
else
    echo "   ❌ PostgreSQL connection failed"
    exit 1
fi

echo ""

# Test Redis
echo "2️⃣  Testing Redis connection..."
if docker exec carpai-redis redis-cli ping &> /dev/null; then
    echo "   ✅ Redis is responding"

    # Check Redis info
    used_memory=$(docker exec carpai-redis redis-cli INFO memory 2>/dev/null | grep "used_memory_human" | cut -d: -f2 | tr -d '\r')
    echo "   📊 Memory usage: $used_memory"
else
    echo "   ❌ Redis connection failed"
    exit 1
fi

echo ""
echo "✅ All database connections successful!"
echo ""
echo "Next steps:"
echo "  1. Update .env file with your configuration"
echo "  2. Run: cargo check --package jcode-auth"
echo "  3. Start implementing TASK-001: Add jcode-auth dependency"
