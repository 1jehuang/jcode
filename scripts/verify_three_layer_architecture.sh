#!/bin/bash
# 验证三层架构部署脚本
# 用法: ./scripts/verify_three_layer_architecture.sh [dev|enterprise]

set -e

PROFILE=${1:-dev}

echo "=================================================="
echo "  CarpAI 三层架构验证"
echo "  Profile: $PROFILE"
echo "  Time: $(date)"
echo "=================================================="
echo ""

# Step 1: 启动Docker Compose服务
echo "📦 Step 1/5: 启动Docker Compose服务..."
if [ "$PROFILE" = "enterprise" ]; then
    docker compose --profile enterprise up -d
else
    docker compose --profile dev up -d
fi

echo "   等待服务启动 (30秒)..."
sleep 30

# Step 2: 检查容器状态
echo ""
echo "🔍 Step 2/5: 检查容器状态..."
docker compose ps

# Step 3: 验证PostgreSQL + pgvector
echo ""
echo "🗄️  Step 3/5: 验证PostgreSQL + pgvector..."
if docker compose exec -T postgres pg_isready -U carpai -d carpai > /dev/null 2>&1; then
    echo "   ✓ PostgreSQL连接成功"

    # 检查pgvector扩展
    if docker compose exec -T postgres psql -U carpai -d carpai -c "SELECT extname FROM pg_extension WHERE extname = 'vector';" | grep -q vector; then
        echo "   ✓ pgvector扩展已启用"
    else
        echo "   ⚠ pgvector扩展未找到,正在安装..."
        docker compose exec -T postgres psql -U carpai -d carpai -c "CREATE EXTENSION IF NOT EXISTS vector;"
    fi
else
    echo "   ✗ PostgreSQL连接失败"
    exit 1
fi

# Step 4: 验证Redis (单节点或集群)
echo ""
echo "💾 Step 4/5: 验证Redis..."
if [ "$PROFILE" = "enterprise" ]; then
    # 检查Redis Cluster
    echo "   检查Redis Cluster..."
    if docker compose exec -T redis-node-1 redis-cli --cluster check redis-node-1:6379 > /dev/null 2>&1; then
        echo "   ✓ Redis Cluster运行正常"
    else
        echo "   ⚠ Redis Cluster未初始化,正在初始化..."
        docker compose exec -T redis-cluster-init sh -c "
            redis-cli --cluster create \
            redis-node-1:6379 redis-node-2:6379 redis-node-3:6379 \
            redis-node-4:6379 redis-node-5:6379 redis-node-6:6379 \
            --cluster-replicas 1 --cluster-yes
        " || true
    fi
else
    # 检查单节点Redis
    if docker compose exec -T redis redis-cli ping | grep -q PONG; then
        echo "   ✓ Redis单节点运行正常"
    else
        echo "   ✗ Redis连接失败"
        exit 1
    fi
fi

# Step 5: 运行系统诊断
echo ""
echo "🏥 Step 5/5: 运行系统诊断..."
echo "   编译doctor命令..."
cargo build --release --bin jcode 2>&1 | tail -5

echo ""
echo "   执行诊断检查..."
export DATABASE_URL="postgresql://carpai:carpai_dev_password@localhost:5432/carpai"
export REDIS_URL="redis://localhost:6379"
export SESSION_STICKY_TTL_SECS=3600
export KV_CACHE_TTL_SECS=3600

./target/release/jcode doctor

# 总结
echo ""
echo "=================================================="
echo "  ✅ 验证完成!"
echo "=================================================="
echo ""
echo "下一步操作:"
echo "  1. 查看Grafana仪表板: http://localhost:3000 (密码: jcode)"
echo "  2. 测试API: curl http://localhost:8081/v1/models"
echo "  3. 查看日志: docker compose logs -f jcode-server"
echo ""

if [ "$PROFILE" = "enterprise" ]; then
    echo "企业功能已启用:"
    echo "  - Redis Cluster (6节点)"
    echo "  - Milvus向量数据库 (端口: 19530)"
    echo "  - Higress网关 (端口: 80, 443, 8080)"
    echo ""
fi

echo "停止服务: docker compose down"
echo "清理数据: docker compose down -v"
