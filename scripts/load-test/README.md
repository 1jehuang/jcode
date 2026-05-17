# CarpAI 压力测试套件

本目录包含用于验证 CarpAI 分布式推理性能的压力测试脚本。

## 📋 前置要求

1. **安装 k6**:
   ```bash
   # macOS
   brew install k6

   # Linux
   sudo apt-get install k6

   # Windows (Chocolatey)
   choco install k6
   ```

2. **启动 CarpAI 服务**:
   ```bash
   cargo run -p jcode-enterprise-server --release
   ```

## 🚀 运行测试

### 基础测试（50 并发，1 分钟）
```bash
k6 run --vus 50 --duration 1m carpai_stress_test.js
```

### 生产级测试（200 并发，5 分钟）
```bash
export CARPAI_URL=http://localhost:8000
export CARPAI_API_KEY=your-api-key-here
k6 run --vus 200 --duration 5m carpai_stress_test.js
```

### 带云报告的高级测试
```bash
k6 cloud carpai_stress_test.js
```

## 📊 关键指标说明

| 指标 | 目标值 | 说明 |
|------|--------|------|
| `http_req_duration p95` | < 500ms | 95% 请求的延迟应低于 500ms |
| `distributed_requests` | > 30% | 大模型请求应触发分布式推理 |
| `scheduling_latency_ms p95` | < 50ms | 调度决策延迟 |
| `kv_cache_hit_rate` | > 80% | KV Cache 命中率 |

## 🔧 自定义测试场景

编辑 `carpai_stress_test.js` 中的 `options.stages` 来调整负载模式：

```javascript
stages: [
  { duration: '30s', target: 10 },   // 预热
  { duration: '2m', target: 200 },   // 爬升
  { duration: '2m', target: 200 },   // 峰值
  { duration: '30s', target: 0 },    // 下降
]
```

## 📈 结果分析

测试完成后会生成 `results.json`，可使用以下工具可视化：
- [k6 Studio](https://k6.io/docs/results-visualization/)
- Grafana + InfluxDB
- Datadog
