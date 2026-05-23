#!/bin/bash
# ============================================================================
# 验证 PostgreSQL + pgvector 配置
# ============================================================================

set -e

DB_URL="${DATABASE_URL:-postgresql://carpai:carpai_dev_password@localhost:5432/carpai}"

echo "=========================================="
echo "PostgreSQL + pgvector 配置验证"
echo "=========================================="
echo ""

# 1. 检查数据库连接
echo "1. 检查数据库连接..."
if command -v psql &> /dev/null; then
    if PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -c "SELECT 1" &> /dev/null; then
        echo "   ✓ 数据库连接成功"
    else
        echo "   ✗ 数据库连接失败，请检查:"
        echo "     - Docker 容器是否运行: docker ps | grep postgres"
        echo "     - 数据库 URL: $DB_URL"
        exit 1
    fi
else
    echo "   ⚠ psql 未安装，跳过直接连接测试"
fi
echo ""

# 2. 检查 pgvector 扩展
echo "2. 检查 pgvector 扩展..."
PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -t -A -c \
    "SELECT COUNT(*) FROM pg_extension WHERE extname = 'vector';" | {
    read count
    if [ "$count" -gt 0 ]; then
        echo "   ✓ pgvector 扩展已安装"
        PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -t -A -c \
            "SELECT extversion FROM pg_extension WHERE extname = 'vector';" | {
            read version
            echo "     版本: $version"
        }
    else
        echo "   ✗ pgvector 扩展未安装"
        echo "     请确保使用 pgvector/pgvector:pg15 Docker 镜像"
        exit 1
    fi
}
echo ""

# 3. 检查向量表是否存在
echo "3. 检查向量表结构..."
TABLES=("code_embeddings" "model_response_cache" "kv_cache_snapshots" "tenant_resource_pools" "session_affinity")

for table in "${TABLES[@]}"; do
    PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -t -A -c \
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_name = '$table';" | {
        read exists
        if [ "$exists" -gt 0 ]; then
            echo "   ✓ 表 $table 存在"
        else
            echo "   ⚠ 表 $table 不存在（可能需要运行迁移）"
        fi
    }
done
echo ""

# 4. 检查 HNSW 索引
echo "4. 检查向量索引..."
PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -t -A -c \
    "SELECT COUNT(*) FROM pg_indexes WHERE indexname LIKE '%embedding%' AND indexdef LIKE '%hnsw%';" | {
    read count
    if [ "$count" -gt 0 ]; then
        echo "   ✓ HNSW 索引已创建 ($count 个)"
    else
        echo "   ⚠ HNSW 索引未找到（可能需要运行迁移）"
    fi
}
echo ""

# 5. 测试向量操作
echo "5. 测试向量插入和搜索..."
PGPASSWORD=carpai_dev_password psql -d "$DB_URL" <<'SQL'
-- 创建临时测试表
CREATE TEMP TABLE test_vectors (
    id SERIAL PRIMARY KEY,
    embedding vector(3)
);

-- 插入测试数据
INSERT INTO test_vectors (embedding) VALUES
    ('[1,0,0]'),
    ('[0,1,0]'),
    ('[0,0,1]');

-- 测试相似度搜索
SELECT
    id,
    embedding <=> '[0.9,0.1,0]' AS distance
FROM test_vectors
ORDER BY distance ASC
LIMIT 1;

-- 清理
DROP TABLE test_vectors;
SQL

if [ $? -eq 0 ]; then
    echo "   ✓ 向量操作测试通过"
else
    echo "   ✗ 向量操作测试失败"
    exit 1
fi
echo ""

# 6. 显示数据库统计信息
echo "6. 数据库统计信息..."
PGPASSWORD=carpai_dev_password psql -d "$DB_URL" -c \
    "SELECT schemaname, tablename, pg_size_pretty(pg_total_relation_size(schemaname||'.'||tablename)) AS size
     FROM pg_tables
     WHERE schemaname = 'public'
     ORDER BY pg_total_relation_size(schemaname||'.'||tablename) DESC;"
echo ""

echo "=========================================="
echo "验证完成！"
echo "=========================================="
