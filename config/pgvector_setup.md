# PostgreSQL + pgvector 配置指南

## 概述

CarpAI 使用 PostgreSQL + pgvector 实现：
1. **业务数据存储** - 用户、审计日志、会话等
2. **向量相似度搜索** - 语义代码搜索、缓存优化
3. **租户隔离** - 资源池管理和智能路由
4. **会话粘性** - KV Cache 感知负载均衡

## 快速启动

### 1. Docker Compose 方式（推荐）

```bash
# 启动 PostgreSQL + pgvector
docker compose up -d postgres

# 验证 pgvector 扩展
docker exec -it carpai-postgres psql -U carpai -d carpai -c "SELECT extname, extversion FROM pg_extension WHERE extname = 'vector';"
```

预期输出：
```
 extname | extversion
---------+------------
 vector  | 0.7.4
```

### 2. 本地安装

#### macOS (Homebrew)
```bash
brew install postgresql@15
brew install pgvector
```

#### Ubuntu/Debian
```bash
sudo apt-get install postgresql-15 postgresql-15-pgvector
```

#### Windows (WSL2)
建议在 WSL2 中安装 Linux 版本，或使用 Docker。

## 数据库迁移

迁移脚本会自动按顺序执行：

```
migrations/
├── 001_create_audit_log.sql          # 审计日志表
├── 002_create_users_and_roles.sql    # 用户和 RBAC
├── 003_create_sessions_and_cache.sql # 会话和协作
├── 004_enable_pgvector.sql           # 启用 pgvector 扩展
└── 005_vector_embeddings.sql         # 向量表和索引
```

手动运行迁移：
```bash
# 使用 sqlx-cli
cargo install sqlx-cli
sqlx migrate run --database-url postgresql://carpai:password@localhost:5432/carpai

# 或使用 Docker 初始化时自动执行
docker compose up -d postgres  # migrations 目录已挂载到 /docker-entrypoint-initdb.d
```

## 核心表结构

### 1. code_embeddings - 语义代码搜索

```sql
CREATE TABLE code_embeddings (
    id UUID PRIMARY KEY,
    file_path TEXT NOT NULL,
    symbol_name TEXT,
    embedding vector(1536),  -- OpenAI ada-002 维度
    metadata JSONB,
    ...
);

-- HNSW 索引用于快速相似度搜索
CREATE INDEX idx_code_embeddings_embedding
ON code_embeddings USING hnsw (embedding vector_cosine_ops);
```

**使用场景**：
- 查找相似代码片段
- 基于语义的代码推荐
- 重构建议

### 2. model_response_cache - 智能响应缓存

```sql
CREATE TABLE model_response_cache (
    model_name VARCHAR(100),
    prompt_embedding vector(1536),
    response_text TEXT,
    cache_hit_count INTEGER,
    ...
);
```

**优势**：
- 基于向量相似度命中缓存（而非精确匹配）
- 减少 30-50% 的重复推理成本
- 支持模糊查询复用

### 3. kv_cache_snapshots - KV Cache 持久化

```sql
CREATE TABLE kv_cache_snapshots (
    instance_id VARCHAR(100),
    model_name VARCHAR(100),
    snapshot_path TEXT,
    storage_tier VARCHAR(20),  -- memory, ssd, object_storage
    size_bytes BIGINT,
    ...
);
```

**功能**：
- 跟踪分布式节点的 KV Cache 状态
- 多层存储策略（热/温/冷）
- 快速恢复推理上下文

### 4. tenant_resource_pools - 租户隔离

```sql
CREATE TABLE tenant_resource_pools (
    tenant_id VARCHAR(100) UNIQUE,
    pool_config JSONB,
    max_concurrent_requests INTEGER,
    allowed_models TEXT[],
    ...
);
```

**用途**：
- 多租户资源配额管理
- 模型访问控制
- 优先级调度

### 5. session_affinity - 会话粘性

```sql
CREATE TABLE session_affinity (
    session_id VARCHAR(100) UNIQUE,
    assigned_node_id VARCHAR(100),
    cache_status VARCHAR(20),  -- hot, warm, cold, expired
    sticky_until TIMESTAMPTZ,
    cache_valid_until TIMESTAMPTZ,
    ...
);
```

**关键特性**：
- 会话与节点绑定（避免缓存失效）
- 缓存 TTL 与粘性有效期严格对齐
- 预刷新机制（过期前重新分配）

## Rust API 使用示例

### 插入向量嵌入

```rust
use jcode_enterprise_server::db::DatabaseManager;

let db = DatabaseManager::new(&config).await?;

// 插入代码向量
let embedding = vec![0.1_f32; 1536]; // 实际从模型获取
let metadata = serde_json::json!({
    "language": "rust",
    "line_count": 42
});

db.upsert_code_embedding(
    "src/main.rs",
    Some("main"),
    &embedding,
    &metadata
).await?;
```

### 向量相似度搜索

```rust
// 查找相似代码
let query_embedding = vec![0.2_f32; 1536];
let results = db.search_similar_code(
    &query_embedding,
    10,     // top-k
    0.3,    // distance threshold
    Some("rust")
).await?;

for result in results {
    println!("Found similar code: {} (score: {})", result.id, result.score);
}
```

### 智能缓存查询

```rust
// 查询缓存的模型响应
let prompt_embedding = compute_embedding(&prompt)?;
if let Some((response, metadata)) = db.find_cached_response(
    "qwen3.6-27b",
    &prompt_embedding,
    0.1  // similarity threshold
).await? {
    println!("Cache hit! Response: {}", response);
} else {
    // 调用模型推理...
}
```

### KV Cache 快照管理

```rust
// 记录快照元数据
db.record_kv_cache_snapshot(
    "node-001",
    "qwen3.6-27b",
    "req-12345",
    "/data/snapshots/node-001-req-12345.bin",
    "ssd",
    1024,   // sequence_length
    64,     // layer_count
    2048,   // size_bytes
    &serde_json::json!({"context": "user_session_abc"}),
    24      // TTL hours
).await?;

// 查找活跃快照
if let Some((path, tier, metadata)) = db.find_active_kv_cache_snapshot(
    "node-001",
    "req-12345"
).await? {
    println!("Found snapshot at: {} (tier: {})", path, tier);
}
```

## 性能优化建议

### 1. HNSW 索引参数调优

```sql
-- 构建时参数（影响索引质量和大小）
CREATE INDEX ... WITH (m = 16, ef_construction = 64);

-- 查询时参数（影响速度和精度）
SET hnsw.ef_search = 40;  -- 默认 40，增大提高精度但降低速度
```

**推荐配置**：
- 小规模 (<100K 向量): `m=16, ef_construction=64, ef_search=40`
- 中规模 (100K-1M): `m=32, ef_construction=128, ef_search=80`
- 大规模 (>1M): 考虑 Milvus

### 2. 向量维度选择

| 嵌入模型 | 维度 | 适用场景 |
|---------|------|---------|
| text-embedding-ada-002 | 1536 | 通用文本、代码 |
| bge-m3 | 1024 | 多语言、跨语言 |
| nomic-embed-text | 768 | 轻量级、快速检索 |

### 3. 缓存 TTL 策略

```sql
-- 高频查询：长 TTL
UPDATE model_response_cache SET ttl_secs = 7200
WHERE cache_hit_count > 10;

-- 清理过期缓存
DELETE FROM model_response_cache WHERE expires_at < NOW();
```

## 监控和维护

### 检查 pgvector 状态

```sql
-- 查看扩展版本
SELECT extname, extversion FROM pg_extension WHERE extname = 'vector';

-- 查看向量表大小
SELECT
    table_name,
    pg_size_pretty(pg_total_relation_size(table_name::text)) AS size
FROM information_schema.tables
WHERE table_schema = 'public'
  AND table_name LIKE '%embedding%' OR table_name LIKE '%cache%';
```

### 索引健康检查

```sql
-- 查看索引使用情况
SELECT
    indexrelname,
    idx_scan,
    idx_tup_read,
    idx_tup_fetch
FROM pg_stat_user_indexes
WHERE indexrelname LIKE '%embedding%';

-- 重建索引（如果需要）
REINDEX INDEX idx_code_embeddings_embedding;
```

### 定期清理

```bash
# 添加 cron 任务清理过期数据
0 2 * * * docker exec carpai-postgres psql -U carpai -d carpai -c \
  "DELETE FROM model_response_cache WHERE expires_at < NOW();"

0 3 * * * docker exec carpai-postgres psql -U carpai -d carpai -c \
  "UPDATE kv_cache_snapshots SET is_active = false WHERE expires_at < NOW();"
```

## 故障排除

### 问题 1: pgvector 扩展未找到

```
ERROR:  could not open extension control file "/usr/share/postgresql/15/extension/vector.control"
```

**解决方案**：
```bash
# Docker 方式
docker compose down
docker compose up -d postgres  # 使用 pgvector/pgvector:pg15 镜像

# 本地安装
sudo apt-get install postgresql-15-pgvector
```

### 问题 2: HNSW 索引构建失败

```
ERROR:  memory required is 268 MB, work_mem is 64 MB
```

**解决方案**：
```sql
-- 临时增加工作内存
SET work_mem = '512MB';
CREATE INDEX ...;
RESET work_mem;
```

### 问题 3: 向量搜索速度慢

**检查清单**：
1. 确认 HNSW 索引存在且有效
2. 调整 `hnsw.ef_search` 参数
3. 检查是否使用了正确的距离度量
4. 考虑增加过滤条件减少扫描范围

## 下一步

完成 pgvector 配置后，继续实施：
1. [ ] 租户隔离层（方案 A 第二步）
2. [ ] 会话粘性管理
3. [ ] 三层负载均衡

详细文档参见：
- [租户隔离设计](../docs/TENANT_ISOLATION.md)
- [负载均衡架构](../docs/LOAD_BALANCING.md)
