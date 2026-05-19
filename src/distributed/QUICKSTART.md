# 分布式集群 - 快速参考

## 🚀 5分钟快速开始

### 1. 创建配置 (30秒)

```bash
cat > ~/.jcode/cluster-config.json <<EOF
{
  "enabled": true,
  "node": {
    "host": "127.0.0.1",
    "port": 9000
  },
  "peers": [],
  "election": {
    "min_quorum_size": 1
  }
}
EOF
```

### 2. 启动服务器 (10秒)

```bash
jcode serve
```

### 3. 查看状态 (10秒)

```bash
curl http://127.0.0.1:9090/api/status | jq
```

### 4. 查看仪表板 (浏览器)

```
http://127.0.0.1:9090/dashboard
```

---

## 📋 常用命令

### CLI 命令

```bash
# 生成配置文件
jcode cluster init-config --output cluster.json

# 启动节点
jcode cluster start --host 127.0.0.1 --port 9000

# 查看状态
jcode cluster status

# 列出节点
jcode cluster list-nodes
```

### API 端点

```bash
# Prometheus 指标
curl http://127.0.0.1:9090/metrics

# 健康检查
curl http://127.0.0.1:9090/health

# 集群状态
curl http://127.0.0.1:9090/api/status

# 节点列表
curl http://127.0.0.1:9090/api/nodes
```

### 日志级别

```bash
# 所有集群日志
RUST_LOG=cluster=info jcode serve

# 只看选举
RUST_LOG=cluster.election=debug jcode serve

# 只看心跳
RUST_LOG=cluster.heartbeat=debug jcode serve

# 完整调试
RUST_LOG=debug jcode serve
```

---

## 🔧 配置示例

### 单节点测试

```json
{
  "enabled": true,
  "node": {"host": "127.0.0.1", "port": 9000},
  "peers": [],
  "election": {"min_quorum_size": 1}
}
```

### 3节点集群

**节点1 (领导者偏好)**:
```json
{
  "enabled": true,
  "node": {
    "host": "127.0.0.1",
    "port": 9000,
    "preferred_role": "Leader"
  },
  "peers": [
    {"address": "127.0.0.1:9001"},
    {"address": "127.0.0.1:9002"}
  ]
}
```

**节点2**:
```json
{
  "enabled": true,
  "node": {"host": "127.0.0.1", "port": 9001},
  "peers": [{"address": "127.0.0.1:9000"}]
}
```

**节点3**:
```json
{
  "enabled": true,
  "node": {"host": "127.0.0.1", "port": 9002},
  "peers": [{"address": "127.0.0.1:9000"}]
}
```

---

## 🐛 故障排除

### 问题: "Address already in use"

```bash
# 查找占用端口的进程
lsof -i :9000

# 杀死进程
kill -9 <PID>
```

### 问题: 节点无法连接

```bash
# 检查防火墙
sudo ufw status

# 允许端口
sudo ufw allow 9000:9999/tcp

# 测试连接
telnet 127.0.0.1 9001
```

### 问题: 选举失败

检查:
1. `min_quorum_size` 是否 <= 健康节点数
2. 所有节点配置是否正确
3. 网络连接是否正常

```bash
# 查看详细选举日志
RUST_LOG=cluster.election=debug jcode serve
```

---

## 📊 Prometheus 查询示例

```promql
# 当前领导者
cluster_is_leader == 1

# 集群大小
cluster_size

# 健康节点比例
cluster_healthy_nodes / cluster_size

# 选举频率
rate(cluster_elections_initiated_total[5m])

# 心跳失败率
rate(cluster_failed_heartbeats_total[5m])

# 运行时间
cluster_uptime_seconds
```

---

## 🧪 测试命令

```bash
# 运行所有测试
cargo test --lib distributed

# 只运行配置测试
cargo test --lib distributed::config::tests

# 只运行指标测试
cargo test --lib distributed::metrics::tests

# 显示测试输出
cargo test --lib distributed -- --nocapture

# 运行单个测试
cargo test test_single_node_initialization
```

---

## 📚 文档索引

| 文档 | 用途 |
|------|------|
| `README.md` | 快速开始 |
| `INTEGRATION_SUMMARY.md` | 集成详情 |
| `TESTING_GUIDE.md` | 测试指南 |
| `FINAL_SUMMARY.md` | 完整总结 |
| `QUICKSTART.md` | 本文档 |

---

## 🔗 相关模块

- `src/distributed/config.rs` - 配置管理
- `src/distributed/service.rs` - 核心服务
- `src/distributed/integration.rs` - 服务器集成
- `src/distributed/metrics.rs` - 监控指标
- `src/distributed/dashboard_api.rs` - HTTP API
- `src/server/server_impl.rs` - 集成位置

---

*快速参考 v0.1.0*
